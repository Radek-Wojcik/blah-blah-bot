//! "Connect account" for GitHub: the OAuth device flow behind Settings > Connectors > GitHub.
//!
//! The client id of the OAuth App is public and comes from the build or the environment
//! (`BBB_GITHUB_CLIENT_ID`); without it the interface says the sign-in is not configured and
//! offers the token field. The token the flow produces goes straight to the credential store
//! under the same entry as a pasted token and is never returned, logged or put in an error.
//! The device code stays here, in memory, between "start" and the polls.
//!
//! The lock on the connection is never held across network I/O: a poll takes the session out,
//! talks to GitHub and puts it back, and a generation counter (bumped by every start and cancel)
//! tells it whether the person cancelled meanwhile, in which case nothing is stored.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use publishers::github::device::{DeviceFlow, DeviceSession, PollOutcome};
use serde::Serialize;

use crate::ipc::{IpcError, IpcResult};
use crate::publish_secrets;
use crate::secrets::{Credentials, SecretStore};

/// The environment variable (and build-time variable) that holds the client id.
pub const CLIENT_ID_ENV: &str = "BBB_GITHUB_CLIENT_ID";

/// The client id of the GitHub OAuth App: the environment of the running application, else
/// the one the application was built with. `None` when neither is set.
pub fn configured_client_id() -> Option<String> {
    let runtime = std::env::var(CLIENT_ID_ENV).ok();
    let built = option_env!("BBB_GITHUB_CLIENT_ID").map(String::from);
    runtime
        .into_iter()
        .chain(built)
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

/// Whether "Connect account" can be offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectStatusDto {
    pub configured: bool,
}

/// What the person sees while approving: the code and the page. The device code is not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStartDto {
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between polls.
    pub interval: u64,
    /// Seconds until the code stops working.
    pub expires_in: u64,
}

/// The state of a connection in progress. There is no token in any of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DevicePollDto {
    Pending,
    #[serde(rename_all = "camelCase")]
    SlowDown {
        interval: u64,
    },
    Denied,
    Expired,
    /// The token is stored. `login` is `None` when GitHub could not be asked whose it is.
    Connected {
        login: Option<String>,
    },
}

/// What the connection is doing: one at a time, a new start replaces the old.
#[derive(Default)]
struct State {
    /// Bumped by every start and cancel; work begun under an older value is discarded.
    generation: u64,
    session: Option<DeviceSession>,
    /// A poll has taken the session out and is talking to GitHub.
    polling: bool,
}

/// The connection in progress, if any.
#[derive(Default)]
pub struct Connection {
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn github_error(message: String) -> IpcError {
    IpcError::new("github", message)
}

fn cancelled() -> IpcError {
    IpcError::new("cancelled", "the GitHub connection was cancelled")
}

pub fn status(client_id: Option<&str>) -> GithubConnectStatusDto {
    GithubConnectStatusDto {
        configured: client_id.is_some_and(DeviceFlow::client_id_is_valid),
    }
}

/// Opens a new attempt: forgets the old one and returns the generation the answer of this
/// start must still carry. The command calls it before the work is handed to another thread,
/// so a cancel that arrives in between is not overwritten.
pub fn begin(conn: &Connection) -> u64 {
    let mut state = lock(&conn.state);
    state.generation += 1;
    state.session = None;
    state.polling = false;
    state.generation
}

/// [`begin`] and [`start_attempt`] in one (the command splits them across a thread).
#[cfg(test)]
fn start(conn: &Connection, flow: &DeviceFlow, now_ms: i64) -> IpcResult<DeviceStartDto> {
    start_attempt(conn, flow, begin(conn), now_ms)
}

/// Asks GitHub for a code and remembers the device code. A cancel or another start while the
/// request is out wins: the answer is then dropped.
pub fn start_attempt(
    conn: &Connection,
    flow: &DeviceFlow,
    generation: u64,
    now_ms: i64,
) -> IpcResult<DeviceStartDto> {
    let code = flow.start().map_err(|e| github_error(e.message))?;
    let dto = DeviceStartDto {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        interval: code.interval_secs,
        expires_in: code.expires_in_secs,
    };
    let mut state = lock(&conn.state);
    if state.generation != generation {
        return Err(cancelled());
    }
    state.session = Some(DeviceSession::new(code, now_ms));
    Ok(dto)
}

/// Polls once (at most as often as GitHub allows) and, on approval, stores the token. `now_ms`
/// is read just before the request goes out, after the session is free.
pub fn poll(
    conn: &Connection,
    flow: &DeviceFlow,
    store: &dyn SecretStore,
    now_ms: impl FnOnce() -> i64,
) -> IpcResult<DevicePollDto> {
    let (generation, mut session) = {
        let mut state = lock(&conn.state);
        if state.polling {
            // The previous poll is still out: nothing new to say.
            return Ok(DevicePollDto::Pending);
        }
        let Some(session) = state.session.take() else {
            return Err(IpcError::new(
                "not_found",
                "no GitHub connection is in progress: start again",
            ));
        };
        state.polling = true;
        (state.generation, session)
    };
    let result = session.poll(flow, now_ms());

    let mut state = lock(&conn.state);
    if state.generation != generation {
        // Cancelled (or restarted) meanwhile: the answer, a token included, is thrown away.
        return Err(cancelled());
    }
    state.polling = false;
    // An error leaves no session behind: the device code is dead weight then.
    let outcome = result.map_err(|e| github_error(e.message))?;
    match outcome {
        PollOutcome::Pending => {
            state.session = Some(session);
            Ok(DevicePollDto::Pending)
        }
        PollOutcome::SlowDown { interval_secs } => {
            state.session = Some(session);
            Ok(DevicePollDto::SlowDown {
                interval: interval_secs,
            })
        }
        PollOutcome::Denied => Ok(DevicePollDto::Denied),
        PollOutcome::Expired => Ok(DevicePollDto::Expired),
        PollOutcome::Token(token) => {
            drop(state);
            // Whose token it is comes first, outside the lock: a cancel during this request
            // must still keep the token out of the store. Not learning the login (the network)
            // does not stop the connection.
            let login = flow.login_of(&token).ok();
            // Storing is local and quick, so it happens under the lock: a cancel either comes
            // before (nothing stored) or after (the person is connected).
            let state = lock(&conn.state);
            if state.generation != generation {
                return Err(cancelled());
            }
            publish_secrets::set_github_token(store, Some(flow.api_base()), token.expose())?;
            drop(state);
            Ok(DevicePollDto::Connected { login })
        }
    }
}

/// Forgets a connection in progress, and makes a start or poll that is out discard its answer.
pub fn cancel(conn: &Connection) {
    let mut state = lock(&conn.state);
    state.generation += 1;
    state.session = None;
    state.polling = false;
}

// ----- commands ----------------------------------------------------------------------------

fn connection() -> &'static Connection {
    static CONNECTION: OnceLock<Connection> = OnceLock::new();
    CONNECTION.get_or_init(Connection::default)
}

/// Milliseconds on a monotonic clock (a clock set back cannot disturb the polling interval).
fn now_ms() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
}

fn flow() -> IpcResult<DeviceFlow> {
    let id = configured_client_id().ok_or_else(|| {
        IpcError::new(
            "not_configured",
            "GitHub sign-in is not configured in this build: use a token instead",
        )
    })?;
    DeviceFlow::new(&id).map_err(|e| IpcError::new("not_configured", e.message))
}

/// Whether "Connect account" is available (a client id is configured).
#[tauri::command]
pub fn github_connect_status() -> GithubConnectStatusDto {
    status(configured_client_id().as_deref())
}

/// Starts the device flow: the code to show and the page to open.
#[tauri::command]
pub async fn github_device_start() -> IpcResult<DeviceStartDto> {
    let flow = flow()?;
    // Synchronously, before the work moves to another thread: see `begin`.
    let generation = begin(connection());
    tauri::async_runtime::spawn_blocking(move || {
        start_attempt(connection(), &flow, generation, now_ms())
    })
    .await
    .map_err(|e| crate::ipc::background_task_error(&e))?
}

/// One poll of the connection in progress.
#[tauri::command]
pub async fn github_device_poll() -> IpcResult<DevicePollDto> {
    let flow = flow()?;
    tauri::async_runtime::spawn_blocking(move || poll(connection(), &flow, &Credentials, now_ms))
        .await
        .map_err(|e| crate::ipc::background_task_error(&e))?
}

/// Drops the connection in progress (the person closed the dialog).
#[tauri::command]
pub fn github_device_cancel() {
    cancel(connection());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemorySecrets;
    use publishers::testing::oauth::{Answer, ACCESS_TOKEN, DEVICE_CODE};
    use publishers::testing::{FakeGithub, FakeOAuth};
    use std::time::Duration;

    struct Rig {
        oauth: FakeOAuth,
        api: FakeGithub,
        flow: DeviceFlow,
        conn: Connection,
        store: MemorySecrets,
    }

    fn rig() -> Rig {
        let api = FakeGithub::start();
        api.require_token(ACCESS_TOKEN);
        let oauth = FakeOAuth::start();
        let flow = DeviceFlow::with_bases("Iv1.testclientid", &oauth.oauth_base(), &api.api_base())
            .unwrap()
            .with_timeouts(Duration::from_secs(2), Duration::from_secs(5));
        Rig {
            oauth,
            api,
            flow,
            conn: Connection::default(),
            store: MemorySecrets::default(),
        }
    }

    fn json_of(dto: &impl Serialize) -> String {
        serde_json::to_string(dto).unwrap()
    }

    #[test]
    fn status_needs_a_client_id() {
        assert!(!status(None).configured);
        assert!(!status(Some("  ")).configured);
        assert!(status(Some("Iv1.abc")).configured);
    }

    #[test]
    fn connecting_stores_the_token_under_the_manual_entry_and_returns_only_the_login() {
        let rig = rig();
        rig.oauth.script(&[Answer::Pending, Answer::Token]);
        let started = start(&rig.conn, &rig.flow, 0).unwrap();
        assert_eq!(started.interval, 5);
        assert!(!json_of(&started).contains(DEVICE_CODE));

        let pending = poll(&rig.conn, &rig.flow, &rig.store, || 5_000).unwrap();
        assert_eq!(pending, DevicePollDto::Pending);
        assert!(rig.store.names().is_empty());

        let done = poll(&rig.conn, &rig.flow, &rig.store, || 10_000).unwrap();
        assert_eq!(
            done,
            DevicePollDto::Connected {
                login: Some("fake-bot".into())
            }
        );
        assert_eq!(
            rig.store.names(),
            vec!["github-token@127.0.0.1".to_string()]
        );
        assert_eq!(
            rig.store.get("github-token@127.0.0.1").as_deref(),
            Some(ACCESS_TOKEN)
        );
        for text in [json_of(&done), format!("{done:?}"), json_of(&started)] {
            assert!(!text.contains(ACCESS_TOKEN), "{text}");
        }
        // The connection is over: another poll has nothing to continue.
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 15_000)
                .unwrap_err()
                .code,
            "not_found"
        );
        // The manual path sees the same entry: GitHub requests carried the token.
        assert!(rig
            .api
            .requests()
            .iter()
            .any(|r| r.header("authorization") == Some(&format!("Bearer {ACCESS_TOKEN}"))));
    }

    #[test]
    fn slow_down_denied_and_expired_reach_the_interface_and_store_nothing() {
        let rig = rig();
        rig.oauth
            .script(&[Answer::SlowDown(10), Answer::Denied, Answer::Expired]);
        start(&rig.conn, &rig.flow, 0).unwrap();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 5_000).unwrap(),
            DevicePollDto::SlowDown { interval: 10 }
        );
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 15_000).unwrap(),
            DevicePollDto::Denied
        );
        assert!(poll(&rig.conn, &rig.flow, &rig.store, || 20_000).is_err());
        start(&rig.conn, &rig.flow, 0).unwrap();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 5_000).unwrap(),
            DevicePollDto::Expired
        );
        assert!(rig.store.names().is_empty());
    }

    #[test]
    fn an_unknown_login_still_counts_as_connected() {
        let rig = rig();
        // The API does not accept the token, so the login cannot be read.
        rig.api.require_token("some-other-token");
        rig.oauth.script(&[Answer::Token]);
        start(&rig.conn, &rig.flow, 0).unwrap();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 5_000).unwrap(),
            DevicePollDto::Connected { login: None }
        );
        assert_eq!(rig.store.names().len(), 1);
    }

    #[test]
    fn errors_and_cancel() {
        let rig = rig();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 0)
                .unwrap_err()
                .code,
            "not_found"
        );
        rig.oauth.script(&[Answer::Error("device_flow_disabled")]);
        start(&rig.conn, &rig.flow, 0).unwrap();
        let err = poll(&rig.conn, &rig.flow, &rig.store, || 5_000).unwrap_err();
        assert_eq!(err.code, "github");
        assert!(!err.message.contains(DEVICE_CODE));
        // The failed poll left no dead session behind.
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, || 10_000)
                .unwrap_err()
                .code,
            "not_found"
        );
        start(&rig.conn, &rig.flow, 0).unwrap();
        cancel(&rig.conn);
        assert!(poll(&rig.conn, &rig.flow, &rig.store, || 15_000).is_err());
    }

    #[test]
    fn status_checks_the_shape_of_the_client_id() {
        assert!(!status(Some("a b")).configured);
        assert!(!status(Some("x\"y")).configured);
        assert!(status(Some(" Iv1.abc ")).configured);
    }

    /// A server whose answers wait until the test lets them go: `gate` names the path that
    /// blocks, `entered` is told when a request is waiting.
    fn gated(
        gate: &'static str,
    ) -> (
        publishers::testing::FakeServer,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        use publishers::testing::{FakeServer, Reply};
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release = Mutex::new(release_rx);
        let server = FakeServer::start(move |req| {
            if req.path == gate {
                let _ = entered_tx.send(());
                let _ = lock(&release).recv();
            }
            match req.path.as_str() {
                "/login/device/code" => Reply::json(
                    200,
                    &serde_json::json!({
                        "device_code": DEVICE_CODE, "user_code": "WDJB-MJHT",
                        "verification_uri": "https://github.com/login/device",
                        "expires_in": 900, "interval": 5,
                    }),
                ),
                _ => Reply::json(200, &serde_json::json!({"access_token": ACCESS_TOKEN})),
            }
        });
        (server, entered_rx, release_tx)
    }

    fn gated_rig(server: &publishers::testing::FakeServer, api: &FakeGithub) -> DeviceFlow {
        DeviceFlow::with_bases("Iv1.testclientid", &server.base_url(), &api.api_base())
            .unwrap()
            .with_timeouts(Duration::from_secs(5), Duration::from_secs(20))
    }

    #[test]
    fn a_cancel_during_a_poll_returns_at_once_and_stores_no_token() {
        let api = FakeGithub::start();
        api.require_token(ACCESS_TOKEN);
        let (server, entered, release) = gated("/login/oauth/access_token");
        let flow = gated_rig(&server, &api);
        let (conn, store) = (Connection::default(), MemorySecrets::default());
        start(&conn, &flow, 0).unwrap();
        std::thread::scope(|scope| {
            let poller = scope.spawn(|| poll(&conn, &flow, &store, || 5_000));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            // Another poll while one is out has nothing to add and does not block.
            assert_eq!(
                poll(&conn, &flow, &store, || 5_000).unwrap(),
                DevicePollDto::Pending
            );
            // The cancel does not wait for the network.
            cancel(&conn);
            release.send(()).unwrap();
            assert_eq!(poller.join().unwrap().unwrap_err().code, "cancelled");
        });
        assert!(store.names().is_empty());
        assert_eq!(
            poll(&conn, &flow, &store, || 10_000).unwrap_err().code,
            "not_found"
        );
    }

    #[test]
    fn a_cancel_during_start_drops_the_answer() {
        let api = FakeGithub::start();
        let (server, entered, release) = gated("/login/device/code");
        let flow = gated_rig(&server, &api);
        let (conn, store) = (Connection::default(), MemorySecrets::default());
        std::thread::scope(|scope| {
            let starter = scope.spawn(|| start(&conn, &flow, 0));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            cancel(&conn);
            release.send(()).unwrap();
            assert_eq!(starter.join().unwrap().unwrap_err().code, "cancelled");
        });
        assert_eq!(
            poll(&conn, &flow, &store, || 5_000).unwrap_err().code,
            "not_found"
        );
    }

    #[test]
    fn a_second_start_wins_over_the_first_that_is_still_out() {
        let api = FakeGithub::start();
        let (server, entered, release) = gated("/login/device/code");
        let flow = gated_rig(&server, &api);
        let conn = Connection::default();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| start(&conn, &flow, 0));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            let second = scope.spawn(|| start(&conn, &flow, 0));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            release.send(()).unwrap();
            release.send(()).unwrap();
            let results = [first.join().unwrap(), second.join().unwrap()];
            let [first, second] = results;
            assert_eq!(first.unwrap_err().code, "cancelled");
            assert!(second.is_ok());
        });
        assert!(lock(&conn.state).session.is_some());
    }

    #[test]
    fn the_wire_shape_is_what_the_interface_reads() {
        assert_eq!(
            json_of(&DevicePollDto::SlowDown { interval: 10 }),
            r#"{"state":"slowDown","interval":10}"#
        );
        assert_eq!(
            json_of(&DevicePollDto::Connected {
                login: Some("a".into())
            }),
            r#"{"state":"connected","login":"a"}"#
        );
        assert_eq!(json_of(&DevicePollDto::Pending), r#"{"state":"pending"}"#);
        assert_eq!(
            json_of(&GithubConnectStatusDto { configured: true }),
            r#"{"configured":true}"#
        );
    }

    #[test]
    fn a_cancel_before_the_worker_starts_is_not_overwritten() {
        let rig = rig();
        let generation = begin(&rig.conn);
        cancel(&rig.conn);
        assert_eq!(
            start_attempt(&rig.conn, &rig.flow, generation, 0)
                .unwrap_err()
                .code,
            "cancelled"
        );
        assert!(lock(&rig.conn.state).session.is_none());
    }

    #[test]
    fn a_cancel_while_the_login_is_asked_stores_no_token() {
        // One server for both bases: the token comes at once, the login request waits.
        let (server, entered, release) = gated("/user");
        let flow =
            DeviceFlow::with_bases("Iv1.testclientid", &server.base_url(), &server.base_url())
                .unwrap()
                .with_timeouts(Duration::from_secs(5), Duration::from_secs(20));
        let (conn, store) = (Connection::default(), MemorySecrets::default());
        start(&conn, &flow, 0).unwrap();
        std::thread::scope(|scope| {
            let poller = scope.spawn(|| poll(&conn, &flow, &store, || 5_000));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            cancel(&conn);
            release.send(()).unwrap();
            assert_eq!(poller.join().unwrap().unwrap_err().code, "cancelled");
        });
        assert!(store.names().is_empty());
    }
}
