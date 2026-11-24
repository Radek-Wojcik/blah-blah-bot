//! A small HTTP server on a loopback port, written on `std::net`, for tests.
//!
//! It is the pattern of the one in `cycle-store` (a hand-written `TcpListener` loop) made
//! reusable: it reads the whole request (with `Content-Length` or chunked), records it,
//! answers from a handler closure, and can be told to misbehave. It exists so that the
//! publishers can be tested against stateful fake services without any async runtime and
//! without ever touching a real host.
//!
//! Faults are queued as rules: the next `n` requests (optionally of one method and path
//! prefix) get a canned answer, lose their connection, or are delayed. [`FakeServer::go_offline`]
//! closes the port, so the client sees a refused connection, and [`FakeServer::go_online`]
//! reopens the same address.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

/// How long a connection may stay silent before it is dropped.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request the server reads, so a runaway client cannot exhaust memory.
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// A request as the server received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    /// The path without the query.
    pub path: String,
    /// The query without the `?`.
    pub query: String,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as JSON, `None` when it is not.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }

    /// The value of a query parameter, undecoded.
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }
}

/// An answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// How the next matching requests misbehave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Answer with this instead of handling the request: nothing changes on the server.
    Reply(Reply),
    /// Close the connection without answering and without handling the request.
    DropBefore,
    /// Handle the request, then close the connection without answering: the effect happened
    /// but the client cannot know it.
    DropAfter,
    /// Wait this long, then handle the request normally (to trigger client timeouts).
    Delay(Duration),
}

struct Rule {
    method: Option<String>,
    path_prefix: Option<String>,
    fault: Fault,
    remaining: usize,
}

type Handler = Box<dyn Fn(&Recorded) -> Reply + Send + Sync>;

struct Shared {
    handler: Handler,
    requests: Mutex<Vec<Recorded>>,
    rules: Mutex<Vec<Rule>>,
}

struct AcceptLoop {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

/// A running fake server.
pub struct FakeServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Mutex<Option<AcceptLoop>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeServer {
    /// Starts a server on a free loopback port. `handler` answers every request that no
    /// fault intercepts; it may keep state of its own behind a mutex.
    pub fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("local address");
        let server = FakeServer {
            addr,
            shared: Arc::new(Shared {
                handler: Box::new(handler),
                requests: Mutex::new(Vec::new()),
                rules: Mutex::new(Vec::new()),
            }),
            accept: Mutex::new(None),
        };
        server.spawn_accept_loop(listener);
        server
    }

    fn spawn_accept_loop(&self, listener: TcpListener) {
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let shared = Arc::clone(&self.shared);
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let shared = Arc::clone(&shared);
                        std::thread::spawn(move || serve(stream, &shared));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            // The listener is dropped here: the port is closed.
        });
        *lock(&self.accept) = Some(AcceptLoop { stop, handle });
    }

    /// `http://127.0.0.1:<port>`, without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Whether the port is open.
    pub fn is_online(&self) -> bool {
        lock(&self.accept).is_some()
    }

    /// Closes the port: new connections are refused until [`go_online`](Self::go_online).
    pub fn go_offline(&self) {
        let running = lock(&self.accept).take();
        if let Some(AcceptLoop { stop, handle }) = running {
            stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
    }

    /// Opens the same address again.
    pub fn go_online(&self) {
        if self.is_online() {
            return;
        }
        // The address may take a moment to become free again on some systems.
        let mut last = None;
        for _ in 0..100 {
            match TcpListener::bind(self.addr) {
                Ok(listener) => {
                    self.spawn_accept_loop(listener);
                    return;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        panic!("cannot reopen {}: {:?}", self.addr, last);
    }

    /// The next `times` requests get `fault`.
    pub fn inject(&self, fault: Fault, times: usize) {
        self.push_rule(None, None, fault, times);
    }

    /// The next `times` requests of `method` whose path starts with `path_prefix` get
    /// `fault`; other requests are not counted.
    pub fn inject_for(&self, method: &str, path_prefix: &str, fault: Fault, times: usize) {
        self.push_rule(
            Some(method.to_ascii_uppercase()),
            Some(path_prefix.to_string()),
            fault,
            times,
        );
    }

    fn push_rule(
        &self,
        method: Option<String>,
        path_prefix: Option<String>,
        fault: Fault,
        times: usize,
    ) {
        lock(&self.shared.rules).push(Rule {
            method,
            path_prefix,
            fault,
            remaining: times,
        });
    }

    pub fn clear_faults(&self) {
        lock(&self.shared.rules).clear();
    }

    /// Every request received so far, faulted ones included, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        lock(&self.shared.requests).clone()
    }

    pub fn request_count(&self) -> usize {
        lock(&self.shared.requests).len()
    }

    /// The requests of `method` whose path starts with `path_prefix`.
    pub fn requests_to(&self, method: &str, path_prefix: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.method.eq_ignore_ascii_case(method) && r.path.starts_with(path_prefix))
            .collect()
    }

    /// Forgets the recorded requests.
    pub fn clear_requests(&self) {
        lock(&self.shared.requests).clear();
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.go_offline();
    }
}

// ----- connection handling ---------------------------------------------------------------

fn serve(mut stream: TcpStream, shared: &Shared) {
    // A socket accepted from a non-blocking listener may inherit the mode.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    lock(&shared.requests).push(request.clone());

    let fault = take_fault(shared, &request);
    let reply = match fault {
        Some(Fault::DropBefore) => return,
        Some(Fault::Reply(reply)) => reply,
        Some(Fault::DropAfter) => {
            let _ = (shared.handler)(&request);
            return;
        }
        Some(Fault::Delay(wait)) => {
            std::thread::sleep(wait);
            (shared.handler)(&request)
        }
        None => (shared.handler)(&request),
    };
    write_reply(&mut stream, &reply);
}

fn take_fault(shared: &Shared, request: &Recorded) -> Option<Fault> {
    let mut rules = lock(&shared.rules);
    for rule in rules.iter_mut() {
        let method_ok = rule
            .method
            .as_ref()
            .is_none_or(|m| m.eq_ignore_ascii_case(&request.method));
        let path_ok = rule
            .path_prefix
            .as_ref()
            .is_none_or(|p| request.path.starts_with(p.as_str()));
        if rule.remaining > 0 && method_ok && path_ok {
            rule.remaining -= 1;
            return Some(rule.fault.clone());
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    let mut rest = buf[head_end + 4..].to_vec();
    let body = if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        read_chunked(stream, &mut rest)?
    } else {
        let length: usize = header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if length > MAX_REQUEST_BYTES {
            return None;
        }
        while rest.len() < length {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(n) => rest.extend_from_slice(&chunk[..n]),
            }
        }
        rest.truncate(length);
        rest
    };

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    Some(Recorded {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Decodes a chunked body; `pending` holds what was already read after the headers.
fn read_chunked(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        // Read until the size line is complete.
        let line_end = loop {
            if let Some(i) = find(pending, b"\r\n") {
                break i;
            }
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
            }
        };
        let size_text = String::from_utf8_lossy(&pending[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        pending.drain(..line_end + 2);
        if size == 0 {
            return Some(body);
        }
        while pending.len() < size + 2 {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
            }
        }
        body.extend_from_slice(&pending[..size]);
        pending.drain(..size + 2);
        if body.len() > MAX_REQUEST_BYTES {
            return None;
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        410 => "Gone",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn write_reply(stream: &mut TcpStream, reply: &Reply) {
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{ErrorKind, HttpClient, Request};
    use store::Failure;

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(3))
    }

    fn echo() -> FakeServer {
        FakeServer::start(|r| Reply::text(200, &format!("{} {}", r.method, r.path)))
    }

    #[test]
    fn it_records_method_path_query_headers_and_body() {
        let server = echo();
        let url = format!("{}/a/b?x=1&y=two", server.base_url());
        let body = serde_json::json!({"k": "v"});
        let reply = client()
            .send(&Request::patch_json(&url, &body).header("X-Test", "yes"))
            .unwrap();
        assert_eq!(reply.text(), "PATCH /a/b");
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].query_param("y"), Some("two"));
        assert_eq!(seen[0].query_param("z"), None);
        assert_eq!(seen[0].header("x-test"), Some("yes"));
        assert_eq!(seen[0].header("content-type"), Some("application/json"));
        assert_eq!(seen[0].json().unwrap(), body);
        assert_eq!(seen[0].header("user-agent"), Some("blah-blah-bot"));
    }

    #[test]
    fn a_large_body_is_read_completely() {
        let server = FakeServer::start(|r| Reply::text(200, &r.body.len().to_string()));
        let bytes = vec![7u8; 3 * 1024 * 1024 + 5];
        let reply = client()
            .send(
                &Request::new(
                    crate::http::Method::Post,
                    format!("{}/up", server.base_url()),
                )
                .bytes("application/octet-stream", bytes),
            )
            .unwrap();
        assert_eq!(reply.text(), (3 * 1024 * 1024 + 5).to_string());
    }

    #[test]
    fn a_chunked_body_is_decoded() {
        let server = echo();
        let mut stream = TcpStream::connect(server.addr()).unwrap();
        stream
            .write_all(
                b"POST /c HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n\
                  5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n",
            )
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert_eq!(server.requests()[0].body_text(), "hello world");
    }

    #[test]
    fn a_reply_fault_answers_without_handling_and_then_clears() {
        let server = echo();
        server.inject(Fault::Reply(Reply::empty(503)), 2);
        let url = format!("{}/x", server.base_url());
        for _ in 0..2 {
            let err = client().send(&Request::get(&url)).unwrap_err();
            assert_eq!(err.status, Some(503));
            assert_eq!(
                err.failure,
                Failure::Transient {
                    retry_after_ms: None
                }
            );
        }
        assert_eq!(client().send(&Request::get(&url)).unwrap().status, 200);
        // Faulted requests are recorded too.
        assert_eq!(server.request_count(), 3);
    }

    #[test]
    fn faults_can_be_limited_to_a_method_and_path() {
        let server = echo();
        server.inject_for("post", "/issues", Fault::Reply(Reply::empty(500)), 1);
        let base = server.base_url();
        // Requests that do not match neither fail nor use up the rule.
        assert!(client()
            .send(&Request::get(format!("{base}/issues")))
            .is_ok());
        assert!(client()
            .send(&Request::get(format!("{base}/other")))
            .is_ok());
        let post = Request::post_json(format!("{base}/issues"), &serde_json::json!({}));
        assert!(client().send(&post).is_err());
        assert!(client().send(&post).is_ok());
        server.inject(Fault::Reply(Reply::empty(500)), 5);
        server.clear_faults();
        assert!(client().send(&Request::get(format!("{base}/x"))).is_ok());
    }

    #[test]
    fn rules_apply_in_order() {
        let server = echo();
        server.inject(Fault::Reply(Reply::empty(500)), 1);
        server.inject(Fault::Reply(Reply::empty(404)), 1);
        let url = format!("{}/x", server.base_url());
        assert_eq!(
            client().send(&Request::get(&url)).unwrap_err().status,
            Some(500)
        );
        assert_eq!(
            client().send(&Request::get(&url)).unwrap_err().status,
            Some(404)
        );
        assert!(client().send(&Request::get(&url)).is_ok());
    }

    #[test]
    fn a_dropped_connection_is_a_transient_connection_error() {
        let server = echo();
        server.inject(Fault::DropBefore, 1);
        let url = format!("{}/x", server.base_url());
        let err = client().send(&Request::get(&url)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Connection, "{err:?}");
        assert!(err.is_offline());
        assert!(matches!(err.failure, Failure::Transient { .. }));
        assert!(client().send(&Request::get(&url)).is_ok());
    }

    #[test]
    fn drop_after_runs_the_handler_but_the_client_sees_a_failure() {
        let hits = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&hits);
        let server = FakeServer::start(move |_| {
            *lock(&counter) += 1;
            Reply::empty(200)
        });
        server.inject(Fault::DropAfter, 1);
        let url = format!("{}/x", server.base_url());
        assert!(client().send(&Request::get(&url)).unwrap_err().is_offline());
        assert_eq!(*lock(&hits), 1);
    }

    #[test]
    fn a_delay_beyond_the_timeout_is_a_timeout() {
        let server = echo();
        server.inject(Fault::Delay(Duration::from_millis(1500)), 1);
        let quick = HttpClient::with_timeouts(Duration::from_secs(1), Duration::from_millis(300));
        let err = quick
            .send(&Request::get(format!("{}/x", server.base_url())))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Timeout, "{err:?}");
        assert!(err.is_offline());
    }

    #[test]
    fn going_offline_refuses_connections_and_going_online_restores_the_address() {
        let server = echo();
        let url = format!("{}/x", server.base_url());
        assert!(client().send(&Request::get(&url)).is_ok());
        server.go_offline();
        assert!(!server.is_online());
        // A closed port is refused at once on most systems and times out on Windows.
        let err = client().send(&Request::get(&url)).unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        assert!(matches!(err.failure, Failure::Transient { .. }));
        server.go_online();
        assert!(server.is_online());
        assert!(client().send(&Request::get(&url)).is_ok());
        // Going online twice is harmless, and the history survives the outage.
        server.go_online();
        assert_eq!(server.request_count(), 2);
    }

    #[test]
    fn a_redirect_is_refused_and_not_followed() {
        let server = FakeServer::start(|r| {
            if r.path == "/start" {
                Reply::empty(302).with_header("Location", "/elsewhere")
            } else {
                Reply::text(200, "reached")
            }
        });
        let err = client()
            .send(&Request::get(format!("{}/start", server.base_url())).bearer("tok"))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Redirect);
        assert_eq!(err.failure, Failure::Permanent);
        assert_eq!(server.request_count(), 1);
    }

    #[test]
    fn the_response_body_is_capped() {
        let server = FakeServer::start(|_| {
            Reply::text(
                200,
                &"x".repeat(crate::http::MAX_RESPONSE_BYTES as usize + 10),
            )
        });
        let err = client()
            .send(&Request::get(format!("{}/big", server.base_url())))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Body, "{err:?}");
    }

    #[test]
    fn dropping_the_server_frees_the_port() {
        let addr = {
            let server = echo();
            server.addr()
        };
        assert!(TcpListener::bind(addr).is_ok());
    }
}
