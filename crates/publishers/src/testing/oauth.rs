//! A scripted fake of GitHub's OAuth device flow (`github.com/login/...`).
//!
//! `POST /login/device/code` answers with a fixed code; `POST /login/oauth/access_token`
//! answers from a queue the test fills ([`FakeOAuth::script`]), and `authorization_pending`
//! once the queue is empty. Every request is recorded by the [`FakeServer`] it derefs to.

use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use super::server::{FakeServer, Recorded, Reply};

pub const DEVICE_CODE: &str = "dev-code-0123456789abcdef";
pub const USER_CODE: &str = "WDJB-MJHT";
pub const ACCESS_TOKEN: &str = "gho_FakeOAuthAccessToken0123456789";

struct State {
    answers: VecDeque<Value>,
    start_answer: Option<Value>,
}

/// What one poll gets.
#[derive(Debug, Clone)]
pub enum Answer {
    Pending,
    SlowDown(u64),
    Denied,
    Expired,
    Token,
    /// Any `error` code of GitHub.
    Error(&'static str),
}

impl Answer {
    fn value(&self) -> Value {
        match self {
            Answer::Pending => json!({"error": "authorization_pending"}),
            Answer::SlowDown(i) => json!({"error": "slow_down", "interval": i}),
            Answer::Denied => json!({"error": "access_denied"}),
            Answer::Expired => json!({"error": "expired_token"}),
            Answer::Token => {
                json!({"access_token": ACCESS_TOKEN, "token_type": "bearer", "scope": "repo"})
            }
            Answer::Error(code) => json!({"error": code}),
        }
    }
}

pub struct FakeOAuth {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}

impl Deref for FakeOAuth {
    type Target = FakeServer;
    fn deref(&self) -> &FakeServer {
        &self.server
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn handle(shared: &Mutex<State>, req: &Recorded) -> Reply {
    if req.method != "POST" || req.header("accept") != Some("application/json") {
        return Reply::empty(404);
    }
    let mut state = lock(shared);
    match req.path.as_str() {
        "/login/device/code" => Reply::json(
            200,
            &state.start_answer.clone().unwrap_or_else(|| {
                json!({
                    "device_code": DEVICE_CODE,
                    "user_code": USER_CODE,
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 5,
                })
            }),
        ),
        "/login/oauth/access_token" => Reply::json(
            200,
            &state
                .answers
                .pop_front()
                .unwrap_or_else(|| Answer::Pending.value()),
        ),
        _ => Reply::empty(404),
    }
}

impl FakeOAuth {
    pub fn start() -> FakeOAuth {
        let state = Arc::new(Mutex::new(State {
            answers: VecDeque::new(),
            start_answer: None,
        }));
        let shared = Arc::clone(&state);
        let server = FakeServer::start(move |req| handle(&shared, req));
        FakeOAuth { server, state }
    }

    /// The OAuth base to give to the client (`http://127.0.0.1:<port>`).
    pub fn oauth_base(&self) -> String {
        self.server.base_url()
    }

    /// The answers of the next polls, in order.
    pub fn script(&self, answers: &[Answer]) {
        lock(&self.state)
            .answers
            .extend(answers.iter().map(Answer::value));
    }

    /// Replaces the answer to "start" with any JSON.
    pub fn set_start_answer(&self, value: Value) {
        lock(&self.state).start_answer = Some(value);
    }
}
