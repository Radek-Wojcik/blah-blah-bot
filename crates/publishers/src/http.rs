//! The HTTP layer every publisher goes through.
//!
//! Three things happen here so that the publishers do not have to repeat them:
//!
//! * **Safety.** A request is refused unless its URL is `https` (plain `http` only for this
//!   machine, for tests and local servers) and carries no credentials. Redirects are never
//!   followed, so a token cannot be handed to another host.
//! * **Classification.** Every failure is either transient (offline, timeout, 5xx, rate
//!   limit: worth another attempt, with the wait the server asked for) or permanent (400,
//!   401, 403 that is not a rate limit, 404, 422: needs the user). The outbox state machine
//!   in the `store` crate takes the result as [`store::Failure`].
//! * **Redaction.** Error texts are built from our own words plus the redacted text of the
//!   underlying error, so a token or a webhook URL never reaches the outbox, the logs or the
//!   interface. See [`redact`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use store::Failure;

/// Largest response body that is read.
pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// How much of an underlying error text is kept.
const DETAIL_CHARS: usize = 200;

/// What went wrong, for the interface and for the host-level pause of the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The host could not be reached (refused, unresolved, reset, dropped).
    Connection,
    Timeout,
    /// Certificate or handshake trouble: needs the user, retrying will not help.
    Tls,
    /// The server answered with a failing status.
    Status,
    /// A rate limit; the wait is in `failure`.
    RateLimited,
    /// The server answered with a redirect, which is refused.
    Redirect,
    /// The response was malformed or too large.
    Body,
    /// The request itself is unacceptable (URL, credentials in the URL, header).
    InvalidRequest,
}

/// A failed request. `message` is safe to store and to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct HttpError {
    pub kind: ErrorKind,
    pub message: String,
    /// The status of the answer, when there was one.
    pub status: Option<u16>,
    /// Worth another attempt or not, with the server's wait for a rate limit.
    pub failure: Failure,
}

impl HttpError {
    fn new(kind: ErrorKind, message: impl Into<String>, failure: Failure) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            failure,
        }
    }

    fn transient(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::new(
            kind,
            message,
            Failure::Transient {
                retry_after_ms: None,
            },
        )
    }

    fn permanent(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::new(kind, message, Failure::Permanent)
    }

    /// The host cannot be reached at all: other operations to it should wait too.
    pub fn is_offline(&self) -> bool {
        matches!(self.kind, ErrorKind::Connection | ErrorKind::Timeout)
    }

    /// The error as the outbox state machine takes it.
    pub fn failure(&self) -> Failure {
        self.failure
    }
}

// ----- addresses -------------------------------------------------------------------------

/// The parts of a URL the checks need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    /// Path and query, starting with `/` (or empty).
    pub path: String,
}

/// Whether `host` is this machine: `localhost`, `127.0.0.0/8` or `::1`.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Splits a URL and applies the safety rules: `https`, or `http` for this machine only, and
/// no user name or password.
pub fn parse_endpoint(url: &str) -> Result<Endpoint, HttpError> {
    let invalid = |why: &str| HttpError::permanent(ErrorKind::InvalidRequest, why.to_string());
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| invalid("the address is not a URL"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(invalid("the address must start with https://"));
    }
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err(invalid("the address must not contain credentials"));
    }
    let (host, port) = split_host_port(authority).ok_or_else(|| invalid("invalid host"))?;
    if host.is_empty() {
        return Err(invalid("the address has no host"));
    }
    if scheme == "http" && !is_loopback_host(&host) {
        return Err(invalid(
            "plain http is only allowed for this machine, use https",
        ));
    }
    Ok(Endpoint {
        scheme,
        host,
        port,
        path: path.to_string(),
    })
}

fn split_host_port(authority: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), Some(port.parse().ok()?))),
        None => Some((authority.to_string(), None)),
    }
}

// ----- requests and responses ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Patch => "PATCH",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Body {
    content_type: String,
    bytes: Vec<u8>,
}

/// A request to build and send with [`HttpClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Body>,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    pub fn post_json(url: impl Into<String>, body: &Value) -> Self {
        Self::new(Method::Post, url).json(body)
    }

    pub fn patch_json(url: impl Into<String>, body: &Value) -> Self {
        Self::new(Method::Patch, url).json(body)
    }

    pub fn json(self, body: &Value) -> Self {
        self.bytes("application/json", body.to_string().into_bytes())
    }

    /// A body of any content type, for example a multipart form.
    pub fn bytes(mut self, content_type: &str, bytes: Vec<u8>) -> Self {
        self.body = Some(Body {
            content_type: content_type.to_string(),
            bytes,
        });
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// `Authorization: Bearer <token>`.
    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", &format!("Bearer {token}"))
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn method(&self) -> Method {
        self.method
    }
}

/// An answer, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<Value, HttpError> {
        serde_json::from_slice(&self.body).map_err(|_| {
            HttpError::permanent(
                ErrorKind::Body,
                "the response is not valid JSON".to_string(),
            )
        })
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The TLS setup of every request: the system library (Schannel on Windows) with the
/// system's own root certificates. ureq's default roots (WebPki) are not compiled in with
/// `native-tls`, and the connector panics on the first `https` request without this.
pub fn tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build()
}

/// The agent as production builds it. Statuses are classified by the caller, and a redirect
/// is an answer, not a detour.
fn build_agent(connect: Duration, total: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(connect))
        .timeout_global(Some(total))
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .tls_config(tls_config())
        .build()
        .into()
}

// ----- the client ------------------------------------------------------------------------

/// Sends requests. Cheap to create; holds the secrets that must be scrubbed from errors.
pub struct HttpClient {
    agent: ureq::Agent,
    secrets: Vec<String>,
    now_ms: fn() -> i64,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self::with_timeouts(Duration::from_secs(10), Duration::from_secs(30))
    }

    pub fn with_timeouts(connect: Duration, total: Duration) -> Self {
        Self {
            agent: build_agent(connect, total),
            secrets: Vec::new(),
            now_ms: system_now_ms,
        }
    }

    /// Registers a value (token, webhook URL, webhook token) that must never appear in an
    /// error text.
    pub fn with_secret(mut self, secret: &str) -> Self {
        if !secret.is_empty() {
            self.secrets.push(secret.to_string());
        }
        self
    }

    /// Replaces the clock, for tests of rate-limit waits.
    pub fn with_clock(mut self, now_ms: fn() -> i64) -> Self {
        self.now_ms = now_ms;
        self
    }

    fn redacted(&self, text: &str) -> String {
        let secrets: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        redact(text, &secrets)
    }

    /// Sends the request and returns the answer whatever its status. Only a request that
    /// gets no answer, or that is refused before it is sent, is an error.
    pub fn send_raw(&self, request: &Request) -> Result<Response, HttpError> {
        let endpoint = parse_endpoint(&request.url)?;
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(&request.url);
        if header(&request.headers, "user-agent").is_none() {
            builder = builder.header("User-Agent", "blah-blah-bot");
        }
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let result = match &request.body {
            Some(body) => {
                let built = builder
                    .header("Content-Type", body.content_type.as_str())
                    .body(body.bytes.clone());
                match built {
                    Ok(r) => self.agent.run(r),
                    Err(_) => return Err(invalid_header()),
                }
            }
            None => match builder.body(()) {
                Ok(r) => self.agent.run(r),
                Err(_) => return Err(invalid_header()),
            },
        };
        let mut response = result.map_err(|e| self.map_error(&e, &endpoint.host))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_ascii_lowercase(), v.to_string()))
            })
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|e| self.map_error(&e, &endpoint.host))?;
        Ok(Response {
            status,
            headers,
            body,
        })
    }

    /// Like [`send_raw`](Self::send_raw), but a status outside 2xx is an error, classified
    /// by [`classify_status`].
    pub fn send(&self, request: &Request) -> Result<Response, HttpError> {
        self.send_refined(request, &|err, _| err)
    }

    /// Like [`send`](Self::send), but `refine` may rewrite the classified error of a failing
    /// answer (a service whose error bodies carry a code that changes what a status means).
    /// The message of what it returns is redacted.
    pub fn send_refined(
        &self,
        request: &Request,
        refine: &dyn Fn(HttpError, &Response) -> HttpError,
    ) -> Result<Response, HttpError> {
        let response = self.send_raw(request)?;
        match classify_status(&response, (self.now_ms)()) {
            None => Ok(response),
            Some(err) => {
                let mut err = refine(err, &response);
                err.message = self.redacted(&err.message);
                Err(err)
            }
        }
    }

    fn map_error(&self, e: &ureq::Error, host: &str) -> HttpError {
        use ureq::Error as E;
        let detail = truncate(&self.redacted(&e.to_string()), DETAIL_CHARS);
        match e {
            E::Timeout(_) => HttpError::transient(
                ErrorKind::Timeout,
                format!("{host} did not answer in time ({detail})"),
            ),
            E::HostNotFound | E::ConnectionFailed | E::Io(_) => HttpError::transient(
                ErrorKind::Connection,
                format!("cannot reach {host} ({detail})"),
            ),
            E::Tls(_) => HttpError::permanent(
                ErrorKind::Tls,
                format!("secure connection to {host} failed ({detail})"),
            ),
            E::BodyExceedsLimit(_) => HttpError::permanent(
                ErrorKind::Body,
                format!("the answer of {host} is too large"),
            ),
            E::BadUri(_) | E::Http(_) => HttpError::permanent(
                ErrorKind::InvalidRequest,
                format!("the request to {host} is invalid ({detail})"),
            ),
            E::RedirectFailed | E::TooManyRedirects => HttpError::permanent(
                ErrorKind::Redirect,
                format!("{host} redirected the request, which is refused"),
            ),
            _ => HttpError::transient(
                ErrorKind::Connection,
                format!("the connection to {host} failed ({detail})"),
            ),
        }
    }
}

fn invalid_header() -> HttpError {
    HttpError::permanent(
        ErrorKind::InvalidRequest,
        "a header of the request is invalid".to_string(),
    )
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

// ----- classification --------------------------------------------------------------------

/// `None` for a 2xx answer, otherwise the error with its classification.
///
/// * 429, and 403 that is a rate limit (a `Retry-After`, an exhausted `x-ratelimit-remaining`
///   or a message that says so): transient, with the wait the server asked for.
/// * 408 and 5xx: transient.
/// * 3xx: permanent, redirects are refused.
/// * Every other status (400, 401, 403, 404, 410, 422, ...): permanent.
pub fn classify_status(response: &Response, now_ms: i64) -> Option<HttpError> {
    let status = response.status;
    if (200..300).contains(&status) {
        return None;
    }
    let message = api_message(&response.body);
    let describe = |what: &str| match &message {
        Some(m) => format!("{what}: {m}"),
        None => what.to_string(),
    };
    let mut err = if (300..400).contains(&status) {
        HttpError::permanent(
            ErrorKind::Redirect,
            describe(&format!("HTTP {status}, redirects are refused")),
        )
    } else if is_rate_limit(response, message.as_deref()) {
        let wait = rate_limit_wait_ms(response, now_ms);
        HttpError::new(
            ErrorKind::RateLimited,
            describe(&format!("HTTP {status}, rate limited")),
            Failure::Transient {
                retry_after_ms: wait,
            },
        )
    } else if status == 408 || status >= 500 {
        HttpError::transient(ErrorKind::Status, describe(&format!("HTTP {status}")))
    } else {
        HttpError::permanent(ErrorKind::Status, describe(&format!("HTTP {status}")))
    };
    err.status = Some(status);
    Some(err)
}

fn is_rate_limit(response: &Response, message: Option<&str>) -> bool {
    match response.status {
        429 => true,
        403 => {
            response.header("retry-after").is_some()
                || response.header("x-ratelimit-remaining") == Some("0")
                || message.is_some_and(|m| m.to_ascii_lowercase().contains("rate limit"))
        }
        _ => false,
    }
}

/// How long the server wants us to wait, in milliseconds, from the first of: `Retry-After`
/// (seconds or an HTTP date), the `retry_after` of a Discord body (seconds, may be
/// fractional), `x-ratelimit-reset-after` (seconds), `x-ratelimit-reset` (epoch seconds,
/// GitHub). `None` when the answer says nothing.
pub fn rate_limit_wait_ms(response: &Response, now_ms: i64) -> Option<i64> {
    if let Some(v) = response.header("retry-after") {
        if let Some(ms) = parse_retry_after(v, now_ms) {
            return Some(ms);
        }
    }
    if let Ok(body) = serde_json::from_slice::<Value>(&response.body) {
        if let Some(secs) = body.get("retry_after").and_then(Value::as_f64) {
            return seconds_to_ms(secs);
        }
    }
    if let Some(secs) = response
        .header("x-ratelimit-reset-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
    {
        return seconds_to_ms(secs);
    }
    if let Some(reset) = response
        .header("x-ratelimit-reset")
        .and_then(|v| v.trim().parse::<f64>().ok())
    {
        return seconds_to_ms(reset - now_ms as f64 / 1000.0);
    }
    None
}

fn seconds_to_ms(secs: f64) -> Option<i64> {
    if secs.is_finite() {
        Some((secs.max(0.0) * 1000.0).ceil() as i64)
    } else {
        None
    }
}

/// Parses a `Retry-After` value: whole seconds, or an HTTP date in the IMF-fixdate form
/// (`Sun, 06 Nov 1994 08:49:37 GMT`). Returns milliseconds from `now_ms`, never negative.
pub fn parse_retry_after(value: &str, now_ms: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(i64::try_from(secs).ok()?.saturating_mul(1000));
    }
    let at = parse_http_date(value)?;
    Some((at * 1000 - now_ms).max(0))
}

/// Unix seconds of an IMF-fixdate.
fn parse_http_date(value: &str) -> Option<i64> {
    let (_, rest) = value.split_once(", ")?;
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let (h, m, s): (i64, i64, i64) = (
        clock.next()?.parse().ok()?,
        clock.next()?.parse().ok()?,
        clock.next()?.parse().ok()?,
    );
    if parts.next()? != "GMT" || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The `message` of a JSON error body, as GitHub and Discord send it, shortened.
pub fn api_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let message = value.get("message")?.as_str()?.trim();
    if message.is_empty() {
        return None;
    }
    Some(truncate(message, DETAIL_CHARS))
}

// ----- redaction -------------------------------------------------------------------------

const MASK: &str = "***";
const TOKEN_PREFIXES: [&str; 6] = ["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
/// Notion token prefixes. Ordinary words can start with `secret_`, so only a run of at least
/// [`LONG_TOKEN_MIN`] token characters after the prefix is masked.
const LONG_TOKEN_PREFIXES: [&str; 2] = ["ntn_", "secret_"];
const LONG_TOKEN_MIN: usize = 20;

/// Removes secrets from a text that may be stored or shown.
///
/// Every occurrence of a value in `secrets` is masked, and so are the shapes secrets take
/// when the value itself is not known: the token part of a Discord webhook path
/// (`/webhooks/<id>/<token>`), the value after `Bearer ` and `Basic `, `access_token=`
/// and `token=` query values, the value after `Bot ` (a Discord bot token), GitHub token
/// prefixes (`ghp_`, `github_pat_`, ...) and Notion token prefixes (`ntn_`, `secret_`).
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, MASK);
    }
    out = mask_webhook_tokens(&out);
    // Short values after "bearer " or "basic " are ordinary words ("bearer of news").
    out = mask_after(&out, "bearer ", 8);
    out = mask_after(&out, "basic ", 8);
    out = mask_after(&out, "access_token=", 1);
    out = mask_after(&out, "token=", 1);
    // A bot token is far longer than any word that follows "bot ".
    out = mask_after(&out, "bot ", LONG_TOKEN_MIN);
    for prefix in TOKEN_PREFIXES {
        out = mask_prefixed(&out, prefix);
    }
    for prefix in LONG_TOKEN_PREFIXES {
        out = mask_prefixed_min(&out, prefix, LONG_TOKEN_MIN);
    }
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '/' | '=')
}

fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

/// Masks the run of token characters after each case-insensitive `marker`, when the run
/// has at least `min_len` characters.
fn mask_after(text: &str, marker: &str, min_len: usize) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(marker) {
        let start = at + found + marker.len();
        out.push_str(&text[at..start]);
        let run = text[start..]
            .char_indices()
            .find(|(_, c)| !is_token_char(*c))
            .map_or(text.len() - start, |(i, _)| i);
        if run >= min_len && &text[start..start + run] != MASK {
            out.push_str(MASK);
        } else {
            out.push_str(&text[start..start + run]);
        }
        at = start + run;
    }
    out.push_str(&text[at..]);
    out
}

/// Masks `<token>` in `/webhooks/<id>/<token>`.
fn mask_webhook_tokens(text: &str) -> String {
    const MARKER: &str = "/webhooks/";
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = text[at..].find(MARKER) {
        let id_start = at + found + MARKER.len();
        out.push_str(&text[at..id_start]);
        let id_len = text[id_start..]
            .find(|c: char| !is_slug_char(c))
            .unwrap_or(text.len() - id_start);
        let after_id = id_start + id_len;
        out.push_str(&text[id_start..after_id]);
        at = after_id;
        if text[after_id..].starts_with('/') {
            let tok_start = after_id + 1;
            let tok_len = text[tok_start..]
                .find(|c: char| !is_slug_char(c))
                .unwrap_or(text.len() - tok_start);
            out.push('/');
            if tok_len > 0 {
                out.push_str(MASK);
            }
            at = tok_start + tok_len;
        }
    }
    out.push_str(&text[at..]);
    out
}

/// Masks `prefix` and the characters that follow it.
fn mask_prefixed(text: &str, prefix: &str) -> String {
    mask_prefixed_min(text, prefix, 0)
}

/// Masks `prefix` and the characters that follow it, when at least `min_run` follow.
fn mask_prefixed_min(text: &str, prefix: &str, min_run: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = text[at..].find(prefix) {
        let start = at + found;
        let after = start + prefix.len();
        let run = text[after..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(text.len() - after);
        if run >= min_run {
            out.push_str(&text[at..start]);
            out.push_str(MASK);
        } else {
            out.push_str(&text[at..after + run]);
        }
        at = after + run;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, headers: &[(&str, &str)], body: &str) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), (*v).to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn failure(status: u16, headers: &[(&str, &str)], body: &str, now_ms: i64) -> Failure {
        classify_status(&response(status, headers, body), now_ms)
            .expect("an error")
            .failure
    }

    const TRANSIENT: Failure = Failure::Transient {
        retry_after_ms: None,
    };

    // ----- TLS -----

    #[test]
    fn production_agent_uses_the_platform_verifier() {
        let agent = build_agent(Duration::from_secs(1), Duration::from_secs(2));
        let tls = agent.config().tls_config();
        assert_eq!(tls.provider(), ureq::tls::TlsProvider::NativeTls);
        assert!(matches!(
            tls.root_certs(),
            ureq::tls::RootCerts::PlatformVerifier
        ));
        assert_eq!(agent.config().max_redirects(), 0);
    }

    /// A real TLS handshake with the production agent: an unauthenticated HEAD of two public
    /// home pages. It reaches the Internet, so it is not run in CI (docs/TESTING.md, section
    /// 14).
    #[test]
    #[ignore = "reaches the Internet: cargo test -p publishers https_handshake -- --ignored"]
    fn https_handshake_with_the_production_agent() {
        let agent = build_agent(Duration::from_secs(10), Duration::from_secs(30));
        for url in ["https://github.com", "https://discord.com"] {
            let response = agent
                .head(url)
                .call()
                .unwrap_or_else(|e| panic!("{url}: {e}"));
            assert!(response.status().as_u16() < 500, "{url}");
        }
    }

    // ----- addresses -----

    #[test]
    fn https_and_loopback_http_are_accepted() {
        let e = parse_endpoint("https://api.github.com/repos/o/r?x=1").unwrap();
        assert_eq!(
            (e.host.as_str(), e.port, e.path.as_str()),
            ("api.github.com", None, "/repos/o/r?x=1")
        );
        for url in [
            "http://127.0.0.1:8080/x",
            "http://localhost/x",
            "http://[::1]:9/x",
            "HTTP://127.0.0.5",
        ] {
            assert!(parse_endpoint(url).is_ok(), "{url}");
        }
        let e = parse_endpoint("http://127.0.0.1:8080").unwrap();
        assert_eq!((e.port, e.path.as_str()), (Some(8080), ""));
    }

    #[test]
    fn other_addresses_are_refused_without_a_network_call() {
        for url in [
            "http://api.github.com/x",
            "http://192.168.0.2/x",
            "ftp://example.com/x",
            "example.com/x",
            "https://user:pass@example.com/x",
            "https://token@example.com/x",
            "https:///x",
            "https://host:notaport/x",
            "http://localhost.example.com/",
        ] {
            let err = parse_endpoint(url).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidRequest, "{url}");
            assert_eq!(err.failure, Failure::Permanent, "{url}");
        }
    }

    #[test]
    fn a_refused_address_is_an_error_of_send() {
        let client = HttpClient::new();
        let err = client
            .send(&Request::get("http://example.com/"))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest);
        assert!(!err.is_offline());
    }

    // ----- classification -----

    #[test]
    fn success_is_not_an_error() {
        for status in [200, 201, 204] {
            assert!(classify_status(&response(status, &[], ""), 0).is_none());
        }
    }

    #[test]
    fn server_trouble_is_transient() {
        for status in [408, 500, 502, 503, 504] {
            assert_eq!(failure(status, &[], "", 0), TRANSIENT, "{status}");
        }
    }

    #[test]
    fn refusals_that_waiting_will_not_fix_are_permanent() {
        for status in [400, 401, 403, 404, 405, 409, 410, 413, 422] {
            assert_eq!(failure(status, &[], "", 0), Failure::Permanent, "{status}");
        }
    }

    #[test]
    fn redirects_are_permanent_and_refused() {
        let err =
            classify_status(&response(301, &[("location", "https://evil.test/")], ""), 0).unwrap();
        assert_eq!(err.kind, ErrorKind::Redirect);
        assert_eq!(err.failure, Failure::Permanent);
        assert!(!err.message.contains("evil.test"));
    }

    #[test]
    fn a_429_is_a_rate_limit_with_the_wait_of_the_server() {
        let err = classify_status(&response(429, &[("Retry-After", "7")], ""), 0).unwrap();
        assert_eq!(err.kind, ErrorKind::RateLimited);
        assert_eq!(
            err.failure,
            Failure::Transient {
                retry_after_ms: Some(7_000)
            }
        );
        // Without any hint the dispatcher's own backoff applies.
        assert_eq!(failure(429, &[], "", 0), TRANSIENT);
    }

    #[test]
    fn github_rate_limits_come_as_403_and_are_transient() {
        // Primary limit: remaining is 0 and the reset is an epoch time.
        let now_ms = 1_700_000_000_000;
        let headers = [
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "1700000090"),
        ];
        assert_eq!(
            failure(
                403,
                &headers,
                r#"{"message":"API rate limit exceeded"}"#,
                now_ms
            ),
            Failure::Transient {
                retry_after_ms: Some(90_000)
            }
        );
        // Secondary limit: a message and Retry-After.
        assert_eq!(
            failure(
                403,
                &[("retry-after", "60")],
                r#"{"message":"You have exceeded a secondary rate limit."}"#,
                now_ms
            ),
            Failure::Transient {
                retry_after_ms: Some(60_000)
            }
        );
        // The message alone is enough to see a limit; the wait is then unknown.
        assert_eq!(
            failure(
                403,
                &[],
                r#"{"message":"You have triggered an abuse RATE LIMIT"}"#,
                0
            ),
            TRANSIENT
        );
        // A plain 403 (no permission, bad scope) is not.
        assert_eq!(
            failure(
                403,
                &[],
                r#"{"message":"Resource not accessible by integration"}"#,
                0
            ),
            Failure::Permanent
        );
        assert_eq!(
            failure(403, &[("x-ratelimit-remaining", "4999")], "", 0),
            Failure::Permanent
        );
    }

    #[test]
    fn discord_rate_limits_carry_a_fractional_wait_in_the_body() {
        let body = r#"{"message":"You are being rate limited.","retry_after":1.5,"global":false}"#;
        assert_eq!(
            failure(429, &[], body, 0),
            Failure::Transient {
                retry_after_ms: Some(1_500)
            }
        );
        assert_eq!(
            failure(429, &[("x-ratelimit-reset-after", "2.25")], "", 0),
            Failure::Transient {
                retry_after_ms: Some(2_250)
            }
        );
    }

    #[test]
    fn the_header_wins_over_the_body_and_the_reset_time() {
        let r = response(
            429,
            &[("retry-after", "3"), ("x-ratelimit-reset", "9999999999")],
            r#"{"retry_after":9}"#,
        );
        assert_eq!(rate_limit_wait_ms(&r, 0), Some(3_000));
        let r = response(429, &[("x-ratelimit-reset", "100")], "");
        // A reset time in the past is no wait at all.
        assert_eq!(rate_limit_wait_ms(&r, 200_000), Some(0));
        assert_eq!(rate_limit_wait_ms(&response(429, &[], "not json"), 0), None);
    }

    #[test]
    fn the_api_message_is_part_of_the_error_text() {
        let err = classify_status(
            &response(422, &[], r#"{"message":"Validation Failed","errors":[]}"#),
            0,
        )
        .unwrap();
        assert_eq!(err.status, Some(422));
        assert_eq!(err.message, "HTTP 422: Validation Failed");
        let err = classify_status(&response(500, &[], "<html>oops</html>"), 0).unwrap();
        assert_eq!(err.message, "HTTP 500");
        assert!(api_message(br#"{"message":""}"#).is_none());
        let long = format!(r#"{{"message":"{}"}}"#, "x".repeat(500));
        assert!(api_message(long.as_bytes()).unwrap().chars().count() < 210);
    }

    // ----- Retry-After -----

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(parse_retry_after("120", 0), Some(120_000));
        assert_eq!(parse_retry_after(" 0 ", 0), Some(0));
        // 1994-11-06 08:49:37 GMT is 784111777 seconds after the epoch.
        let at = 784_111_777_000;
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", at - 10_000),
            Some(10_000)
        );
        // A date in the past means no wait.
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", at + 5_000),
            Some(0)
        );
    }

    #[test]
    fn retry_after_rejects_what_it_cannot_read() {
        for bad in [
            "",
            "soon",
            "-5",
            "1.5",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 25:49:37 GMT",
        ] {
            assert_eq!(parse_retry_after(bad, 0), None, "{bad:?}");
        }
        // A number too large for milliseconds does not overflow.
        assert!(parse_retry_after("18446744073709551615", 0).is_none());
        assert_eq!(
            parse_retry_after("9223372036854775", 0),
            Some(9_223_372_036_854_775_000)
        );
    }

    #[test]
    fn civil_dates_convert_to_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    // ----- redaction -----

    #[test]
    fn known_secrets_are_masked_everywhere() {
        let text = "failed with abc123secret in https://x/abc123secret and ABC123SECRET";
        assert_eq!(
            redact(text, &["abc123secret"]),
            "failed with *** in https://x/*** and ABC123SECRET"
        );
        assert_eq!(redact("nothing here", &[""]), "nothing here");
    }

    #[test]
    fn a_webhook_token_is_masked_even_when_it_is_not_known() {
        let url = "https://discord.com/api/webhooks/123456789012345678/AbC-dEf_ghi123?wait=true";
        assert_eq!(
            redact(&format!("POST {url} failed"), &[]),
            "POST https://discord.com/api/webhooks/123456789012345678/***?wait=true failed"
        );
        assert_eq!(
            redact("http://127.0.0.1:5/api/webhooks/1/tok", &[]),
            "http://127.0.0.1:5/api/webhooks/1/***"
        );
        // No token part: nothing to mask.
        assert_eq!(
            redact("https://discord.com/api/webhooks/123", &[]),
            "https://discord.com/api/webhooks/123"
        );
        assert_eq!(redact("/webhooks/", &[]), "/webhooks/");
    }

    #[test]
    fn authorization_values_and_token_shapes_are_masked() {
        assert_eq!(
            redact("header Authorization: Bearer abc.DEF-123 rejected", &[]),
            "header Authorization: Bearer *** rejected"
        );
        assert_eq!(
            redact("Authorization: basic eC1hY2Nlc3M6dG9r end", &[]),
            "Authorization: basic *** end"
        );
        assert_eq!(
            redact("GET /x?access_token=SeCrEt&per_page=1", &[]),
            "GET /x?access_token=***&per_page=1"
        );
        assert_eq!(
            redact(
                "token ghp_abcdefABCDEF0123456789 and github_pat_11AA_bb",
                &[]
            ),
            "token *** and ***"
        );
        // Ordinary words stay readable.
        assert_eq!(
            redact("the token expired, bearer of bad news", &[]),
            "the token expired, bearer of bad news"
        );
    }

    #[test]
    fn notion_and_bot_tokens_are_masked_by_their_shape() {
        let ntn = concat!("ntn_", "1234567890abcdefABCDEF1234567890abcdef");
        let legacy = concat!("secret_", "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789ABCDEFG");
        assert_eq!(
            redact(&format!("the token {ntn} was refused"), &[]),
            "the token *** was refused"
        );
        assert_eq!(
            redact(&format!("old {legacy}, then ntn_{}.", "x".repeat(20)), &[]),
            "old ***, then ***."
        );
        let bot = concat!("MTExMTExMTExMTExMTExMTEx", ".GaBcDe.", "Abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(
            redact(&format!("Authorization: Bot {bot} rejected"), &[]),
            "Authorization: Bot *** rejected"
        );
        assert_eq!(
            redact(&format!("authorization: bot {bot}"), &[]),
            "authorization: bot ***"
        );
        // Words and short values stay readable.
        for text in [
            "the secret_x of the plan",
            "a secret_ is not a token",
            "ntn_short",
            "this secret_key_is_a_name",
            "the bot is missing a permission",
            "Bot rejected",
            "a robot arm",
            "the bot Abcdefghij failed",
        ] {
            assert_eq!(redact(text, &[]), text);
        }
    }

    #[test]
    fn redacting_twice_changes_nothing() {
        let once = redact("Bearer abc and /webhooks/1/tok and ghp_xyz", &["abc"]);
        assert_eq!(redact(&once, &["abc"]), once);
    }

    #[test]
    fn errors_of_the_client_carry_no_registered_secret() {
        // The server does not exist: the port is closed again before the request.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let secret = "s3cr3t-token-value";
        let client = HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
            .with_secret(secret);
        let err = client
            .send(&Request::get(format!("http://127.0.0.1:{port}/hook/{secret}")).bearer(secret))
            .unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        assert!(matches!(err.failure, Failure::Transient { .. }));
        assert!(!err.message.contains(secret), "{}", err.message);
        assert!(err.message.contains("127.0.0.1"));
    }
}
