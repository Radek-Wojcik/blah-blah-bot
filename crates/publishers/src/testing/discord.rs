//! A stateful fake of a Discord webhook.
//!
//! It implements what the publisher needs from `POST /api/webhooks/{id}/{token}` (JSON or
//! `multipart/form-data` with `payload_json` and `files[n]`, with `?wait=true`) and
//! `GET /api/webhooks/{id}/{token}`, and enforces the limits of the Discord documentation
//! that the publisher has to respect: 2000 characters of content, 10 embeds, 10 attachments
//! and 10 MiB of attachments per message, and a message that is not empty. Errors have the
//! shape and codes Discord uses (`50035` invalid form body, `50006` empty message, `10015`
//! unknown webhook, `50027` invalid token, `40005` entity too large).
//!
//! The length of the content is counted in Unicode characters.

use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use super::server::{FakeServer, Recorded, Reply};

/// Most characters of message content.
pub const MAX_CONTENT_CHARS: usize = 2000;
/// Most embeds in one message.
pub const MAX_EMBEDS: usize = 10;
/// Most attachments in one message.
pub const MAX_FILES: usize = 10;
/// Most bytes of attachments in one message.
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// An attachment as the fake received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub filename: String,
    pub content_type: Option<String>,
    pub size: usize,
}

/// A message the fake accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub id: String,
    pub content: String,
    /// The `allowed_mentions` object as sent; `None` when the request had none.
    pub allowed_mentions: Option<Value>,
    pub embeds: usize,
    pub files: Vec<StoredFile>,
    /// The whole JSON payload.
    pub payload: Value,
}

struct State {
    webhook_id: String,
    webhook_token: String,
    deleted: bool,
    messages: Vec<StoredMessage>,
    next_id: u64,
}

/// The fake Discord. Derefs to its [`FakeServer`] for faults, offline mode and the log of
/// requests.
pub struct FakeDiscord {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}

impl Deref for FakeDiscord {
    type Target = FakeServer;
    fn deref(&self) -> &FakeServer {
        &self.server
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `429` with the wait in the body, in `Retry-After` and in `x-ratelimit-reset-after`.
pub fn rate_limited(retry_after_secs: f64) -> Reply {
    Reply::json(
        429,
        &json!({"message": "You are being rate limited.", "retry_after": retry_after_secs, "global": false}),
    )
    .with_header("Retry-After", &format!("{}", retry_after_secs.ceil() as u64))
    .with_header("x-ratelimit-remaining", "0")
    .with_header("x-ratelimit-reset-after", &format!("{retry_after_secs}"))
}

impl FakeDiscord {
    /// Starts the fake with one webhook.
    pub fn start() -> FakeDiscord {
        let state = Arc::new(Mutex::new(State {
            webhook_id: "100000000000000001".to_string(),
            webhook_token: "fake-Webhook_token-AbC123".to_string(),
            deleted: false,
            messages: Vec::new(),
            next_id: 200_000_000_000_000_001,
        }));
        let shared = Arc::clone(&state);
        let server = FakeServer::start(move |req| handle(&shared, req));
        FakeDiscord { server, state }
    }

    /// The webhook URL to store as the secret, `http://127.0.0.1:<port>/api/webhooks/<id>/<token>`.
    pub fn webhook_url(&self) -> String {
        let s = lock(&self.state);
        format!(
            "{}/api/webhooks/{}/{}",
            self.server.base_url(),
            s.webhook_id,
            s.webhook_token
        )
    }

    pub fn webhook_token(&self) -> String {
        lock(&self.state).webhook_token.clone()
    }

    /// The webhook is deleted on Discord: it answers 404 "Unknown Webhook" from now on.
    pub fn delete_webhook(&self) {
        lock(&self.state).deleted = true;
    }

    /// The messages accepted so far, oldest first.
    pub fn messages(&self) -> Vec<StoredMessage> {
        lock(&self.state).messages.clone()
    }
}

// ----- the API ---------------------------------------------------------------------------

fn api_error(status: u16, code: u32, message: &str) -> Reply {
    Reply::json(status, &json!({"message": message, "code": code}))
}

fn invalid_form(field: &str, rule: &str, message: &str) -> Reply {
    Reply::json(
        400,
        &json!({
            "message": "Invalid Form Body",
            "code": 50035,
            "errors": {field: {"_errors": [{"code": rule, "message": message}]}},
        }),
    )
}

fn handle(shared: &Mutex<State>, req: &Recorded) -> Reply {
    let mut state = lock(shared);
    let segments: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    let ["api", "webhooks", id, token] = segments.as_slice() else {
        return api_error(404, 0, "404: Not Found");
    };
    if state.deleted || *id != state.webhook_id {
        return api_error(404, 10015, "Unknown Webhook");
    }
    if *token != state.webhook_token {
        return api_error(401, 50027, "Invalid Webhook Token");
    }
    match req.method.as_str() {
        "GET" => Reply::json(
            200,
            &json!({
                "id": state.webhook_id,
                "type": 1,
                "name": "Fake webhook",
                "channel_id": "300000000000000001",
                "guild_id": "400000000000000001",
            }),
        ),
        "POST" => post_message(&mut state, req),
        _ => api_error(405, 0, "405: Method Not Allowed"),
    }
}

fn post_message(state: &mut State, req: &Recorded) -> Reply {
    let content_type = req.header("content-type").unwrap_or("");
    let (payload, files) = if content_type.starts_with("multipart/form-data") {
        match parse_multipart(content_type, &req.body) {
            Some(parsed) => parsed,
            None => return api_error(400, 50035, "Invalid Form Body"),
        }
    } else if content_type.starts_with("application/json") {
        match req.json() {
            Some(v) if v.is_object() => (v, Vec::new()),
            _ => return api_error(400, 50109, "The request body contains invalid JSON."),
        }
    } else {
        return api_error(400, 50035, "Invalid Form Body");
    };

    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let embeds = payload
        .get("embeds")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if content.chars().count() > MAX_CONTENT_CHARS {
        return invalid_form(
            "content",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 2000 or fewer in length.",
        );
    }
    if embeds > MAX_EMBEDS {
        return invalid_form(
            "embeds",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 10 or fewer in length.",
        );
    }
    if files.len() > MAX_FILES {
        return invalid_form(
            "files",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 10 or fewer in length.",
        );
    }
    if files.iter().map(|f| f.size).sum::<usize>() > MAX_FILE_BYTES {
        return api_error(413, 40005, "Request entity too large");
    }
    if content.trim().is_empty() && embeds == 0 && files.is_empty() {
        return api_error(400, 50006, "Cannot send an empty message");
    }

    let id = state.next_id.to_string();
    state.next_id += 1;
    let message = StoredMessage {
        id: id.clone(),
        content: content.clone(),
        allowed_mentions: payload.get("allowed_mentions").cloned(),
        embeds,
        files: files.clone(),
        payload: payload.clone(),
    };
    state.messages.push(message);

    if req.query_param("wait") == Some("true") {
        Reply::json(
            200,
            &json!({
                "id": id,
                "type": 0,
                "content": content,
                "channel_id": "300000000000000001",
                "webhook_id": state.webhook_id,
                "attachments": files.iter().enumerate().map(|(i, f)| json!({
                    "id": format!("{i}"), "filename": f.filename, "size": f.size,
                })).collect::<Vec<_>>(),
                "embeds": payload.get("embeds").cloned().unwrap_or_else(|| json!([])),
            }),
        )
    } else {
        Reply::empty(204)
    }
}

// ----- multipart -------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < from + needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The value of `key="..."` in a header such as `Content-Disposition`.
fn quoted_param(header: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=\"");
    let start = header.find(&marker)? + marker.len();
    let end = header[start..].find('"')? + start;
    Some(header[start..end].to_string())
}

/// Reads `payload_json` and the `files[n]` parts of a multipart body.
fn parse_multipart(content_type: &str, body: &[u8]) -> Option<(Value, Vec<StoredFile>)> {
    let boundary = content_type
        .split(';')
        .filter_map(|p| p.trim().strip_prefix("boundary="))
        .next()?
        .trim_matches('"');
    let delimiter = format!("--{boundary}").into_bytes();
    let mut payload = None;
    let mut files = Vec::new();
    let mut at = find(body, &delimiter, 0)?;
    loop {
        let after = at + delimiter.len();
        if body[after..].starts_with(b"--") {
            break;
        }
        // The part starts after the CRLF that ends the delimiter line.
        let head_start = find(body, b"\r\n", after)? + 2;
        let head_end = find(body, b"\r\n\r\n", head_start - 2)?.max(head_start - 2);
        let head =
            String::from_utf8_lossy(&body[head_start..head_end.max(head_start)]).into_owned();
        let data_start = head_end + 4;
        let next = find(body, &delimiter, data_start)?;
        // The data ends with the CRLF before the next delimiter.
        let data_end = next.checked_sub(2)?.max(data_start);
        let data = &body[data_start..data_end];

        let disposition = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-disposition"))?
            .to_string();
        let name = quoted_param(&disposition, "name")?;
        if name == "payload_json" {
            payload = Some(serde_json::from_slice::<Value>(data).ok()?);
        } else if name.starts_with("files[") {
            files.push(StoredFile {
                filename: quoted_param(&disposition, "filename")?,
                content_type: head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
                    .map(|l| l["content-type:".len()..].trim().to_string()),
                size: data.len(),
            });
        }
        at = next;
    }
    Some((payload.unwrap_or_else(|| json!({})), files))
}

#[cfg(test)]
pub(crate) fn multipart_body(boundary: &str, payload: &Value, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut part = |headers: String, data: &[u8]| {
        body.extend_from_slice(format!("--{boundary}\r\n{headers}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    };
    part(
        "Content-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json"
            .to_string(),
        payload.to_string().as_bytes(),
    );
    for (i, (name, data)) in files.iter().enumerate() {
        part(
            format!(
                "Content-Disposition: form-data; name=\"files[{i}]\"; filename=\"{name}\"\r\nContent-Type: image/png"
            ),
            data,
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::super::server::Fault;
    use super::*;
    use crate::http::{ErrorKind, HttpClient, Method, Request};
    use std::time::Duration;
    use store::Failure;

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(10))
    }

    fn send_json(
        dc: &FakeDiscord,
        body: Value,
    ) -> Result<crate::http::Response, crate::http::HttpError> {
        client().send(&Request::post_json(
            format!("{}?wait=true", dc.webhook_url()),
            &body,
        ))
    }

    fn send_multipart(
        dc: &FakeDiscord,
        payload: &Value,
        files: &[(&str, &[u8])],
    ) -> Result<crate::http::Response, crate::http::HttpError> {
        let boundary = "bbb-test-boundary";
        client().send(
            &Request::new(Method::Post, format!("{}?wait=true", dc.webhook_url())).bytes(
                &format!("multipart/form-data; boundary={boundary}"),
                multipart_body(boundary, payload, files),
            ),
        )
    }

    #[test]
    fn a_json_message_is_stored_and_echoed_with_wait() {
        let dc = FakeDiscord::start();
        let reply = send_json(
            &dc,
            json!({"content": "Meeting of 2026-11-13", "allowed_mentions": {"parse": []}}),
        )
        .unwrap();
        assert_eq!(reply.status, 200);
        let v = reply.json().unwrap();
        assert_eq!(v["content"], "Meeting of 2026-11-13");
        let stored = dc.messages();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, v["id"].as_str().unwrap());
        assert_eq!(stored[0].allowed_mentions, Some(json!({"parse": []})));
        assert!(stored[0].files.is_empty());
        // Without wait the answer is empty.
        let quiet = client()
            .send(&Request::post_json(
                dc.webhook_url(),
                &json!({"content": "x"}),
            ))
            .unwrap();
        assert_eq!((quiet.status, quiet.body.len()), (204, 0));
        assert_eq!(dc.messages().len(), 2);
    }

    #[test]
    fn content_is_limited_to_2000_characters_not_bytes() {
        let dc = FakeDiscord::start();
        assert!(send_json(&dc, json!({"content": "a".repeat(2000)})).is_ok());
        // 2000 two-byte characters are still 2000 characters.
        assert!(send_json(&dc, json!({"content": "ł".repeat(2000)})).is_ok());
        for text in ["a".repeat(2001), "ł".repeat(2001)] {
            let err = send_json(&dc, json!({"content": text})).unwrap_err();
            assert_eq!((err.status, err.failure), (Some(400), Failure::Permanent));
            assert!(err.message.contains("Invalid Form Body"), "{}", err.message);
        }
        assert_eq!(dc.messages().len(), 2);
    }

    #[test]
    fn an_empty_message_is_refused() {
        let dc = FakeDiscord::start();
        for body in [
            json!({}),
            json!({"content": "   "}),
            json!({"content": "", "embeds": []}),
        ] {
            let err = send_json(&dc, body).unwrap_err();
            assert_eq!(err.status, Some(400));
            assert!(err.message.contains("empty"), "{}", err.message);
        }
        // Files alone make a message.
        assert!(send_multipart(&dc, &json!({}), &[("a.png", b"png")]).is_ok());
    }

    #[test]
    fn embeds_are_limited_to_ten() {
        let dc = FakeDiscord::start();
        let embeds = |n: usize| {
            (0..n)
                .map(|i| json!({"title": format!("e{i}")}))
                .collect::<Vec<_>>()
        };
        assert!(send_json(&dc, json!({"embeds": embeds(10)})).is_ok());
        assert_eq!(
            send_json(&dc, json!({"embeds": embeds(11)}))
                .unwrap_err()
                .status,
            Some(400)
        );
        assert_eq!(dc.messages()[0].embeds, 10);
    }

    #[test]
    fn a_multipart_message_records_the_payload_and_each_file() {
        let dc = FakeDiscord::start();
        let payload = json!({"content": "With pictures", "allowed_mentions": {"parse": []}});
        let reply =
            send_multipart(&dc, &payload, &[("one.png", b"11111"), ("two.png", b"22")]).unwrap();
        assert_eq!(
            reply.json().unwrap()["attachments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let m = &dc.messages()[0];
        assert_eq!(m.content, "With pictures");
        assert_eq!(m.payload, payload);
        assert_eq!(
            m.files,
            [
                StoredFile {
                    filename: "one.png".into(),
                    content_type: Some("image/png".into()),
                    size: 5
                },
                StoredFile {
                    filename: "two.png".into(),
                    content_type: Some("image/png".into()),
                    size: 2
                },
            ]
        );
    }

    #[test]
    fn file_data_that_contains_the_delimiter_or_line_breaks_is_not_cut() {
        let dc = FakeDiscord::start();
        let data = b"line\r\nbreak\r\n\r\n--not-the-boundary\r\nend";
        send_multipart(&dc, &json!({"content": "x"}), &[("odd.bin", data)]).unwrap();
        assert_eq!(dc.messages()[0].files[0].size, data.len());
    }

    #[test]
    fn at_most_ten_files_per_message() {
        let dc = FakeDiscord::start();
        let ten: Vec<(&str, &[u8])> = (0..10).map(|_| ("a.png", &b"x"[..])).collect();
        assert!(send_multipart(&dc, &json!({"content": "ten"}), &ten).is_ok());
        let eleven: Vec<(&str, &[u8])> = (0..11).map(|_| ("a.png", &b"x"[..])).collect();
        let err = send_multipart(&dc, &json!({"content": "eleven"}), &eleven).unwrap_err();
        assert_eq!(err.status, Some(400));
        assert_eq!(dc.messages().len(), 1);
    }

    #[test]
    fn attachments_are_limited_to_ten_mebibytes_per_message() {
        let dc = FakeDiscord::start();
        let exactly = vec![0u8; MAX_FILE_BYTES];
        assert!(send_multipart(&dc, &json!({"content": "limit"}), &[("a.bin", &exactly)]).is_ok());
        // Two files that together are one byte over.
        let half = vec![0u8; MAX_FILE_BYTES / 2];
        let one_more = vec![0u8; MAX_FILE_BYTES / 2 + 1];
        let err = send_multipart(
            &dc,
            &json!({"content": "over"}),
            &[("a.bin", &half), ("b.bin", &one_more)],
        )
        .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(413), Failure::Permanent));
        assert_eq!(dc.messages().len(), 1);
    }

    #[test]
    fn a_broken_multipart_body_is_a_bad_request() {
        let dc = FakeDiscord::start();
        let err = client()
            .send(
                &Request::new(Method::Post, dc.webhook_url())
                    .bytes("multipart/form-data; boundary=zz", b"garbage".to_vec()),
            )
            .unwrap_err();
        assert_eq!(err.status, Some(400));
        let err = client()
            .send(
                &Request::new(Method::Post, dc.webhook_url())
                    .bytes("application/json", b"{x".to_vec()),
            )
            .unwrap_err();
        assert_eq!(err.status, Some(400));
    }

    #[test]
    fn the_webhook_must_exist_and_the_token_must_match() {
        let dc = FakeDiscord::start();
        let good = dc.webhook_url();
        let wrong_token = good.replace(&dc.webhook_token(), "wrong");
        let err = client()
            .send(&Request::post_json(wrong_token, &json!({"content": "x"})))
            .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(401), Failure::Permanent));
        assert!(err.message.contains("Invalid Webhook Token"));
        let wrong_id = good.replace("100000000000000001", "100000000000000002");
        let err = client()
            .send(&Request::post_json(wrong_id, &json!({"content": "x"})))
            .unwrap_err();
        assert_eq!(err.status, Some(404));
        assert!(err.message.contains("Unknown Webhook"));
        assert!(dc.messages().is_empty());
        dc.delete_webhook();
        assert_eq!(
            client().send(&Request::get(&good)).unwrap_err().status,
            Some(404)
        );
    }

    #[test]
    fn a_get_describes_the_webhook_without_posting() {
        let dc = FakeDiscord::start();
        let info = client()
            .send(&Request::get(dc.webhook_url()))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(info["id"], "100000000000000001");
        assert!(info.get("token").is_none());
        assert!(dc.messages().is_empty());
    }

    #[test]
    fn a_rate_limit_carries_the_wait_in_every_form() {
        let dc = FakeDiscord::start();
        dc.inject(Fault::Reply(rate_limited(1.5)), 1);
        let err = send_json(&dc, json!({"content": "x"})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::RateLimited);
        // Retry-After (whole seconds, rounded up) wins over the fractional body value.
        assert_eq!(
            err.failure,
            Failure::Transient {
                retry_after_ms: Some(2_000)
            }
        );
        assert!(dc.messages().is_empty());
        assert!(send_json(&dc, json!({"content": "x"})).is_ok());
    }

    #[test]
    fn a_lost_answer_leaves_the_message_posted() {
        let dc = FakeDiscord::start();
        dc.inject(Fault::DropAfter, 1);
        let err = send_json(&dc, json!({"content": "once"})).unwrap_err();
        assert!(err.is_offline());
        // The reason a retry has to resume from a record of what was sent.
        assert_eq!(dc.messages().len(), 1);
    }

    #[test]
    fn offline_and_back() {
        let dc = FakeDiscord::start();
        dc.go_offline();
        assert!(send_json(&dc, json!({"content": "x"}))
            .unwrap_err()
            .is_offline());
        dc.go_online();
        assert!(send_json(&dc, json!({"content": "x"})).is_ok());
    }

    #[test]
    fn the_webhook_token_never_appears_in_a_client_error() {
        let dc = FakeDiscord::start();
        let url = dc.webhook_url();
        let token = dc.webhook_token();
        let client = client().with_secret(&url).with_secret(&token);
        dc.go_offline();
        let err = client
            .send(&Request::post_json(url, &json!({"content": "x"})))
            .unwrap_err();
        assert!(!err.message.contains(&token), "{}", err.message);
        dc.go_online();
        dc.inject(
            Fault::Reply(Reply::json(
                500,
                &json!({"message": format!("oops {token}")}),
            )),
            1,
        );
        let err = client
            .send(&Request::post_json(
                dc.webhook_url(),
                &json!({"content": "x"}),
            ))
            .unwrap_err();
        assert!(!err.message.contains(&token), "{}", err.message);
    }
}
