//! "Connect account" for GitHub: the OAuth device flow behind Settings > Connectors > GitHub.
//!
//! The client id of the OAuth App is public and comes from the build or the environment
//! (`BBB_GITHUB_CLIENT_ID`); without it the interface says the sign-in is not configured and
//! offers the token field. The token the flow produces goes straight to the credential store
//! under the same entry as a pasted token and is never returned, logged or put in an error.
//! The device code stays here, in memory, between "start" and the polls.

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// The connection in progress, if any: one at a time, a new start replaces the old.
#[derive(Default)]
pub struct Connection {
    session: Mutex<Option<DeviceSession>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn github_error(message: String) -> IpcError {
    IpcError::new("github", message)
}

pub fn status(client_id: Option<&str>) -> GithubConnectStatusDto {
    GithubConnectStatusDto {
        configured: client_id.is_some_and(|id| !id.trim().is_empty()),
    }
}

/// Asks GitHub for a code and remembers the device code.
pub fn start(conn: &Connection, flow: &DeviceFlow, now_ms: i64) -> IpcResult<DeviceStartDto> {
    let code = flow.start().map_err(|e| github_error(e.message))?;
    let dto = DeviceStartDto {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        interval: code.interval_secs,
        expires_in: code.expires_in_secs,
    };
    *lock(&conn.session) = Some(DeviceSession::new(code, now_ms));
    Ok(dto)
}

/// Polls once (at most as often as GitHub allows) and, on approval, stores the token.
pub fn poll(
    conn: &Connection,
    flow: &DeviceFlow,
    store: &dyn SecretStore,
    now_ms: i64,
) -> IpcResult<DevicePollDto> {
    let mut guard = lock(&conn.session);
    let Some(session) = guard.as_mut() else {
        return Err(IpcError::new(
            "not_found",
            "no GitHub connection is in progress: start again",
        ));
    };
    let outcome = session
        .poll(flow, now_ms)
        .map_err(|e| github_error(e.message))?;
    match outcome {
        PollOutcome::Pending => Ok(DevicePollDto::Pending),
        PollOutcome::SlowDown { interval_secs } => Ok(DevicePollDto::SlowDown {
            interval: interval_secs,
        }),
        PollOutcome::Denied => {
            *guard = None;
            Ok(DevicePollDto::Denied)
        }
        PollOutcome::Expired => {
            *guard = None;
            Ok(DevicePollDto::Expired)
        }
        PollOutcome::Token(token) => {
            // The device code is used up whatever happens next.
            *guard = None;
            publish_secrets::set_github_token(store, Some(flow.api_base()), token.expose())?;
            // The token is valid and stored; failing to learn whose it is does not undo that.
            let login = flow.login_of(&token).ok();
            Ok(DevicePollDto::Connected { login })
        }
    }
}

/// Forgets a connection in progress.
pub fn cancel(conn: &Connection) {
    *lock(&conn.session) = None;
}

// ----- commands ----------------------------------------------------------------------------

fn connection() -> &'static Connection {
    static CONNECTION: OnceLock<Connection> = OnceLock::new();
    CONNECTION.get_or_init(Connection::default)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
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
    tauri::async_runtime::spawn_blocking(move || start(connection(), &flow, now_ms()))
        .await
        .map_err(|e| crate::ipc::background_task_error(&e))?
}

/// One poll of the connection in progress.
#[tauri::command]
pub async fn github_device_poll() -> IpcResult<DevicePollDto> {
    let flow = flow()?;
    tauri::async_runtime::spawn_blocking(move || poll(connection(), &flow, &Credentials, now_ms()))
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

        let pending = poll(&rig.conn, &rig.flow, &rig.store, 5_000).unwrap();
        assert_eq!(pending, DevicePollDto::Pending);
        assert!(rig.store.names().is_empty());

        let done = poll(&rig.conn, &rig.flow, &rig.store, 10_000).unwrap();
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
            poll(&rig.conn, &rig.flow, &rig.store, 15_000)
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
            poll(&rig.conn, &rig.flow, &rig.store, 5_000).unwrap(),
            DevicePollDto::SlowDown { interval: 10 }
        );
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, 15_000).unwrap(),
            DevicePollDto::Denied
        );
        assert!(poll(&rig.conn, &rig.flow, &rig.store, 20_000).is_err());
        start(&rig.conn, &rig.flow, 0).unwrap();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, 5_000).unwrap(),
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
            poll(&rig.conn, &rig.flow, &rig.store, 5_000).unwrap(),
            DevicePollDto::Connected { login: None }
        );
        assert_eq!(rig.store.names().len(), 1);
    }

    #[test]
    fn errors_and_cancel() {
        let rig = rig();
        assert_eq!(
            poll(&rig.conn, &rig.flow, &rig.store, 0).unwrap_err().code,
            "not_found"
        );
        rig.oauth.script(&[Answer::Error("device_flow_disabled")]);
        start(&rig.conn, &rig.flow, 0).unwrap();
        let err = poll(&rig.conn, &rig.flow, &rig.store, 5_000).unwrap_err();
        assert_eq!(err.code, "github");
        assert!(!err.message.contains(DEVICE_CODE));
        cancel(&rig.conn);
        assert!(poll(&rig.conn, &rig.flow, &rig.store, 10_000).is_err());
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
}
