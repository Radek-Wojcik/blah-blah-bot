use super::*;
use crate::testing::oauth::{Answer, ACCESS_TOKEN, DEVICE_CODE, USER_CODE};
use crate::testing::{FakeGithub, FakeOAuth};

fn flow(oauth: &FakeOAuth, api: &FakeGithub) -> DeviceFlow {
    DeviceFlow::with_bases("Iv1.testclientid", &oauth.oauth_base(), &api.api_base())
        .unwrap()
        .with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
}

fn servers() -> (FakeOAuth, FakeGithub) {
    let api = FakeGithub::start();
    api.require_token(ACCESS_TOKEN);
    (FakeOAuth::start(), api)
}

fn polls(oauth: &FakeOAuth) -> usize {
    oauth.requests_to("POST", "/login/oauth/access_token").len()
}

#[test]
fn start_sends_the_client_id_and_scope_and_returns_the_code() {
    let (oauth, api) = servers();
    let code = flow(&oauth, &api).start().unwrap();
    assert_eq!(code.user_code, USER_CODE);
    assert_eq!(code.verification_uri, "https://github.com/login/device");
    assert_eq!(code.interval_secs, 5);
    assert_eq!(code.expires_in_secs, 900);
    let sent = oauth.requests_to("POST", "/login/device/code");
    assert_eq!(sent.len(), 1);
    let body = sent[0].json().unwrap();
    assert_eq!(body["client_id"], "Iv1.testclientid");
    assert_eq!(body["scope"], "repo");
}

#[test]
fn pending_then_success_gives_the_token_and_the_login() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::Pending, Answer::Token]);
    let mut session = DeviceSession::new(flow.start().unwrap(), 0);
    // Too early: GitHub is not asked.
    assert_eq!(session.poll(&flow, 1_000).unwrap(), PollOutcome::Pending);
    assert_eq!(polls(&oauth), 0);
    assert_eq!(session.poll(&flow, 5_000).unwrap(), PollOutcome::Pending);
    assert_eq!(polls(&oauth), 1);
    // The interval is kept: not before 5 s after the last poll.
    assert_eq!(session.poll(&flow, 9_999).unwrap(), PollOutcome::Pending);
    assert_eq!(polls(&oauth), 1);
    let PollOutcome::Token(token) = session.poll(&flow, 10_000).unwrap() else {
        panic!("expected a token");
    };
    assert_eq!(token.expose(), ACCESS_TOKEN);
    assert_eq!(flow.login_of(&token).unwrap(), "fake-bot");
    let body = oauth.requests_to("POST", "/login/oauth/access_token")[0]
        .json()
        .unwrap();
    assert_eq!(body["device_code"], DEVICE_CODE);
    assert_eq!(body["grant_type"], GRANT_TYPE);
}

#[test]
fn slow_down_lengthens_the_interval() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::SlowDown(10), Answer::Pending]);
    let mut session = DeviceSession::new(flow.start().unwrap(), 0);
    assert_eq!(
        session.poll(&flow, 5_000).unwrap(),
        PollOutcome::SlowDown { interval_secs: 10 }
    );
    assert_eq!(session.interval_secs(), 10);
    // 5 s later is too early now.
    assert_eq!(session.poll(&flow, 10_000).unwrap(), PollOutcome::Pending);
    assert_eq!(polls(&oauth), 1);
    assert_eq!(session.poll(&flow, 15_000).unwrap(), PollOutcome::Pending);
    assert_eq!(polls(&oauth), 2);
}

#[test]
fn slow_down_without_an_interval_adds_five_seconds() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::SlowDown(0)]);
    let mut session = DeviceSession::new(flow.start().unwrap(), 0);
    assert_eq!(
        session.poll(&flow, 5_000).unwrap(),
        PollOutcome::SlowDown { interval_secs: 10 }
    );
}

#[test]
fn denied_and_expired_are_reported() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::Denied, Answer::Expired]);
    let mut session = DeviceSession::new(flow.start().unwrap(), 0);
    assert_eq!(session.poll(&flow, 5_000).unwrap(), PollOutcome::Denied);
    assert_eq!(session.poll(&flow, 10_000).unwrap(), PollOutcome::Expired);
    // Past its own lifetime the code is expired without asking.
    let before = polls(&oauth);
    assert_eq!(session.poll(&flow, 901_000).unwrap(), PollOutcome::Expired);
    assert_eq!(polls(&oauth), before);
}

#[test]
fn unknown_errors_become_our_own_sentence() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::Error("device_flow_disabled")]);
    let err = flow.poll(DEVICE_CODE).unwrap_err();
    assert!(err.message.contains("not turned on"), "{err}");
    oauth.set_start_answer(serde_json::json!({"error": "incorrect_client_credentials"}));
    assert!(flow.start().unwrap_err().message.contains("client id"));
}

#[test]
fn only_github_hosts_and_this_machine_are_allowed() {
    for oauth in [
        "https://evil.example",
        "https://github.com.evil.example",
        "https://api.github.com",
        "http://github.com",
        "https://user@github.com",
    ] {
        assert!(
            DeviceFlow::with_bases("id", oauth, DEFAULT_API).is_err(),
            "{oauth}"
        );
    }
    for api in [
        "https://evil.example",
        "https://github.com",
        "http://api.github.com",
    ] {
        assert!(
            DeviceFlow::with_bases("id", DEFAULT_OAUTH_BASE, api).is_err(),
            "{api}"
        );
    }
    assert!(DeviceFlow::new("Iv1.abc").is_ok());
    assert!(DeviceFlow::with_bases("id", "http://127.0.0.1:1", "http://127.0.0.1:2").is_ok());
    let long = "a".repeat(65);
    for id in ["", "  ", "a b", "x\"y", long.as_str()] {
        assert!(DeviceFlow::new(id).is_err(), "{id:?}");
    }
}

#[test]
fn a_verification_page_outside_github_is_refused() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    for uri in ["https://evil.example/login/device", "javascript:alert(1)"] {
        oauth.set_start_answer(serde_json::json!({
            "device_code": DEVICE_CODE, "user_code": USER_CODE,
            "verification_uri": uri, "expires_in": 900, "interval": 5,
        }));
        assert!(flow.start().is_err(), "{uri}");
    }
}

#[test]
fn the_token_and_the_device_code_are_not_in_any_debug_text() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.script(&[Answer::Token]);
    let code = flow.start().unwrap();
    let session = DeviceSession::new(code.clone(), 0);
    let outcome = DeviceSession::new(code.clone(), 0)
        .poll(&flow, 5_000)
        .unwrap();
    let PollOutcome::Token(ref token) = outcome else {
        panic!("expected a token");
    };
    let texts = [
        format!("{code:?}"),
        format!("{session:?}"),
        format!("{outcome:?}"),
        format!("{token:?}"),
    ];
    for text in texts {
        assert!(!text.contains(ACCESS_TOKEN), "{text}");
        assert!(!text.contains(DEVICE_CODE), "{text}");
    }
}

#[test]
fn a_server_that_is_down_is_an_error_without_secrets() {
    let (oauth, api) = servers();
    let flow = flow(&oauth, &api);
    oauth.go_offline();
    let err = flow.start().unwrap_err();
    assert!(!err.message.contains(DEVICE_CODE));
    assert!(!err.message.is_empty());
}
