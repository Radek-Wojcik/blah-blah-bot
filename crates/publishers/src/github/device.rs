//! GitHub's OAuth **device flow**: the user approves the application in their browser with a
//! short code, and the application polls until GitHub hands over a token. No client secret is
//! involved, so the client id is the only configuration and it is public.
//!
//! * `POST {oauth}/login/device/code` starts it (scope `repo`);
//! * `POST {oauth}/login/oauth/access_token` is polled at the interval GitHub asked for, and
//!   `slow_down` raises that interval;
//! * `GET {api}/user` tells whose account it is.
//!
//! Only `github.com` and `api.github.com` are talked to (plain `http` on this machine for the
//! fake servers of the tests), so a client id and a token can never be sent to another host.
//! Neither the access token nor the device code appears in a `Debug` text or an error.

use std::time::Duration;

use serde_json::{json, Value};

use super::{Github, DEFAULT_API};
use crate::http::{is_loopback_host, parse_endpoint, HttpClient, HttpError, Request};

/// Where the device flow talks to.
pub const DEFAULT_OAUTH_BASE: &str = "https://github.com";
/// The scope asked for: creating private repositories and issues needs `repo`.
pub const SCOPE: &str = "repo";
const OAUTH_HOST: &str = "github.com";
const API_HOST: &str = "api.github.com";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Added to the interval by `slow_down` when GitHub does not name a longer one.
const SLOW_DOWN_STEP_SECS: u64 = 5;
/// The shortest wait between two polls, whatever GitHub says.
const MIN_INTERVAL_SECS: u64 = 1;

/// Why a step of the device flow failed. The text is safe to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DeviceError {
    pub message: String,
}

impl DeviceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<HttpError> for DeviceError {
    fn from(e: HttpError) -> Self {
        Self::new(e.message)
    }
}

/// An OAuth access token. Its `Debug` text hides the value.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

/// What GitHub answered to "start": what the user sees, and the device code the polling needs.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode {
    device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
    pub expires_in_secs: u64,
}

impl DeviceCode {
    pub fn device_code(&self) -> &str {
        &self.device_code
    }
}

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCode")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("interval_secs", &self.interval_secs)
            .field("expires_in_secs", &self.expires_in_secs)
            .finish()
    }
}

/// The answer of one poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// The user has not approved yet.
    Pending,
    /// Poll less often: the interval GitHub named (0 when it named none).
    SlowDown {
        interval_secs: u64,
    },
    Denied,
    Expired,
    Token(AccessToken),
}

/// The client of the device flow.
pub struct DeviceFlow {
    oauth_base: String,
    api_base: String,
    client_id: String,
    connect: Duration,
    total: Duration,
}

/// `url` normalised, when it is `https` on `host` (or any loopback address, for tests).
fn allowed_base(url: &str, host: &str) -> Result<String, DeviceError> {
    let url = url.trim().trim_end_matches('/');
    let endpoint = parse_endpoint(url).map_err(|e| DeviceError::new(e.message))?;
    let ok = endpoint.host.eq_ignore_ascii_case(host) || is_loopback_host(&endpoint.host);
    if !ok || endpoint.path.contains(['?', '#']) {
        return Err(DeviceError::new(format!(
            "only {host} is allowed for the GitHub sign-in"
        )));
    }
    Ok(url.to_string())
}

impl DeviceFlow {
    /// The public GitHub.
    pub fn new(client_id: &str) -> Result<Self, DeviceError> {
        Self::with_bases(client_id, DEFAULT_OAUTH_BASE, DEFAULT_API)
    }

    /// Other base addresses, for tests: they must be on this machine or the real hosts.
    pub fn with_bases(
        client_id: &str,
        oauth_base: &str,
        api_base: &str,
    ) -> Result<Self, DeviceError> {
        let client_id = client_id.trim();
        if client_id.is_empty()
            || client_id.len() > 64
            || !client_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(DeviceError::new("the GitHub client id is not valid"));
        }
        Ok(Self {
            oauth_base: allowed_base(oauth_base, OAUTH_HOST)?,
            api_base: allowed_base(api_base, API_HOST)?,
            client_id: client_id.to_string(),
            connect: Duration::from_secs(10),
            total: Duration::from_secs(30),
        })
    }

    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.connect = connect;
        self.total = total;
        self
    }

    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    fn client(&self) -> HttpClient {
        HttpClient::with_timeouts(self.connect, self.total)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, DeviceError> {
        let request = Request::post_json(format!("{}{path}", self.oauth_base), body)
            .header("Accept", "application/json");
        Ok(self.client().send(&request)?.json()?)
    }

    /// Asks GitHub for a device code.
    pub fn start(&self) -> Result<DeviceCode, DeviceError> {
        let value = self.post(
            "/login/device/code",
            &json!({"client_id": self.client_id, "scope": SCOPE}),
        )?;
        if let Some(code) = value.get("error").and_then(Value::as_str) {
            return Err(DeviceError::new(error_text(code)));
        }
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let number = |key: &str| value.get(key).and_then(Value::as_u64);
        let bad = || DeviceError::new("GitHub's answer does not describe a device code");
        let verification_uri = text("verification_uri").ok_or_else(bad)?;
        // The page the user is sent to must be GitHub's own.
        allowed_base(&verification_uri, OAUTH_HOST)
            .map_err(|_| DeviceError::new("GitHub named a page that is not on github.com"))?;
        Ok(DeviceCode {
            device_code: text("device_code").ok_or_else(bad)?,
            user_code: text("user_code").ok_or_else(bad)?,
            verification_uri,
            interval_secs: number("interval").unwrap_or(5).max(MIN_INTERVAL_SECS),
            expires_in_secs: number("expires_in").unwrap_or(900),
        })
    }

    /// One poll. The caller keeps to the interval (see [`DeviceSession`]).
    pub fn poll(&self, device_code: &str) -> Result<PollOutcome, DeviceError> {
        let value = self.post(
            "/login/oauth/access_token",
            &json!({
                "client_id": self.client_id,
                "device_code": device_code,
                "grant_type": GRANT_TYPE,
            }),
        )?;
        if let Some(token) = value.get("access_token").and_then(Value::as_str) {
            if token.is_empty() {
                return Err(DeviceError::new("GitHub sent an empty token"));
            }
            return Ok(PollOutcome::Token(AccessToken(token.to_string())));
        }
        match value.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Ok(PollOutcome::Pending),
            Some("slow_down") => Ok(PollOutcome::SlowDown {
                interval_secs: value.get("interval").and_then(Value::as_u64).unwrap_or(0),
            }),
            Some("access_denied") => Ok(PollOutcome::Denied),
            Some("expired_token") => Ok(PollOutcome::Expired),
            Some(code) => Err(DeviceError::new(error_text(code))),
            None => Err(DeviceError::new(
                "GitHub's answer has no token and no error",
            )),
        }
    }

    /// The login of the account the token belongs to.
    pub fn login_of(&self, token: &AccessToken) -> Result<String, DeviceError> {
        let api = Github::with_client(self.client(), &self.api_base, token.expose())?;
        Ok(api.current_user()?)
    }
}

/// Our own sentence for an error code of GitHub (the raw text is never passed on).
fn error_text(code: &str) -> &'static str {
    match code {
        "device_flow_disabled" => {
            "the device flow is not turned on for this GitHub OAuth App: use a token instead"
        }
        "incorrect_client_credentials" | "unauthorized_client" => {
            "GitHub does not know this application (check the client id): use a token instead"
        }
        "incorrect_device_code" | "bad_verification_code" => {
            "GitHub no longer accepts this code: start again"
        }
        "unsupported_grant_type" | "invalid_request" => "GitHub refused the request: start again",
        _ => "GitHub refused the sign-in: use a token instead",
    }
}

#[cfg(test)]
mod tests;

/// Where a connection is in time: keeps to the interval, grows it on `slow_down` and knows
/// when the code expires. Time comes from the caller (milliseconds), so tests need no sleeping.
#[derive(Clone)]
pub struct DeviceSession {
    code: DeviceCode,
    interval_secs: u64,
    next_poll_ms: i64,
    expires_ms: i64,
}

impl std::fmt::Debug for DeviceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSession")
            .field("code", &self.code)
            .field("interval_secs", &self.interval_secs)
            .finish()
    }
}

impl DeviceSession {
    pub fn new(code: DeviceCode, now_ms: i64) -> Self {
        let interval_secs = code.interval_secs;
        Self {
            interval_secs,
            next_poll_ms: now_ms + interval_secs as i64 * 1000,
            expires_ms: now_ms + code.expires_in_secs as i64 * 1000,
            code,
        }
    }

    pub fn code(&self) -> &DeviceCode {
        &self.code
    }

    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    /// Polls when the interval has passed; a call that comes too early answers `Pending`
    /// without asking GitHub, so a hurried caller cannot get the application rate limited.
    pub fn poll(&mut self, flow: &DeviceFlow, now_ms: i64) -> Result<PollOutcome, DeviceError> {
        if now_ms >= self.expires_ms {
            return Ok(PollOutcome::Expired);
        }
        if now_ms < self.next_poll_ms {
            return Ok(PollOutcome::Pending);
        }
        let outcome = flow.poll(self.code.device_code())?;
        if let PollOutcome::SlowDown { interval_secs } = outcome {
            self.interval_secs = interval_secs.max(self.interval_secs + SLOW_DOWN_STEP_SECS);
            self.next_poll_ms = now_ms + self.interval_secs as i64 * 1000;
            return Ok(PollOutcome::SlowDown {
                interval_secs: self.interval_secs,
            });
        }
        self.next_poll_ms = now_ms + self.interval_secs as i64 * 1000;
        Ok(outcome)
    }
}
