mod common;

use std::sync::Arc;

use common::{FlakyStore, Harness, block_on};
use leapp_core::{Error, Field, SessionKind, SessionStatus};

const DEVICE: &str = "arn:aws:iam::123456789012:mfa/dev";

#[tokio::test]
async fn a_session_with_an_mfa_device_sends_the_code() {
    let h = Harness::new().await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some(format!(" {DEVICE} "));
    let id = h.leapp.create_iam_user(req).unwrap();
    h.prompter.mfa_answers(vec![Some("123456")]);

    h.leapp.start(id).await.unwrap();

    let requests = h.fake.requests();
    let call = requests
        .iter()
        .rfind(|r| r.action == "GetSessionToken")
        .unwrap();
    let param = |k: &str| call.params.get(k).map(String::as_str);
    assert_eq!(param("SerialNumber"), Some(DEVICE));
    assert_eq!(param("TokenCode"), Some("123456"));
    assert_eq!(param("DurationSeconds"), Some("36000"));
    assert_eq!(h.prompter.mfa_asked(), ["dev"]);
}

#[tokio::test]
async fn a_cancelled_mfa_code_fails_the_start() {
    let h = Harness::new().await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some(DEVICE.to_string());
    let id = h.leapp.create_iam_user(req).unwrap();
    h.write_credentials("[other]\nregion = us-west-2\n");
    let before = h.credentials_text();
    h.prompter.mfa_answers(vec![None]);

    let e = h.leapp.start(id).await.unwrap_err();

    assert_eq!(e.to_string(), "Missing Multi Factor Authentication code");
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
    assert_eq!(h.fake.count("GetSessionToken"), 0);
    assert_eq!(h.credentials_text(), before);
}

#[tokio::test]
async fn a_session_without_an_mfa_device_never_asks() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    assert!(h.prompter.mfa_asked().is_empty());
    let call = &h.fake.requests()[0];
    assert!(!call.params.contains_key("SerialNumber"));
    assert!(!call.params.contains_key("TokenCode"));
}

#[tokio::test]
async fn an_empty_mfa_device_is_stored_as_none() {
    let h = Harness::new().await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some("   ".to_string());

    let id = h.leapp.create_iam_user(req).unwrap();

    let ws = h.leapp.workspace();
    match &ws.session(id).unwrap().kind {
        SessionKind::IamUser { mfa_device, .. } => assert_eq!(*mfa_device, None),
    }
    let saved: serde_json::Value = serde_json::from_str(&h.workspace_text()).unwrap();
    let kind = &saved["sessions"][0]["kind"];
    assert!(
        kind.get("mfa_device").is_some_and(|d| d.is_null()),
        "{kind}"
    );
}

#[tokio::test]
async fn an_input_error_names_its_field() {
    let h = Harness::new().await;
    let field = |req| match h.leapp.create_iam_user(req).unwrap_err() {
        Error::InvalidInput { field, .. } => field,
        other => panic!("unexpected error {other:?}"),
    };
    let mut r = h.iam_user("dev", "default");
    r.name = String::new();
    assert_eq!(field(r), Field::Name);
    let mut r = h.iam_user("dev", "default");
    r.region = "xx-nowhere-1".into();
    assert_eq!(field(r), Field::Region);
    assert_eq!(Field::MfaDevice.to_string(), "mfa_device");
}

/// A start that reaches the MFA step after `shutdown` began asks no question: the app is closing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_after_shutdown_began_asks_no_mfa_code() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some(DEVICE.to_string());
    let id = h.leapp.create_iam_user(req).unwrap();
    h.prompter.mfa_answers(vec![Some("123456")]);

    // The start reads the keys just before the MFA question; that read waits at the gate.
    let gate = store.block_get(&format!("iam-user-{id}-access-key-id"));
    let leapp = &h.leapp;
    let started = std::thread::scope(|scope| {
        let starter = scope.spawn(|| block_on(leapp.start(id)));
        gate.wait_blocked();
        // On its own thread: this thread runs the test runtime.
        scope
            .spawn(|| block_on(leapp.shutdown()))
            .join()
            .unwrap()
            .unwrap();
        gate.open();
        starter.join().unwrap()
    });

    assert!(matches!(started, Err(Error::Closing)), "{started:?}");
    assert!(h.prompter.mfa_asked().is_empty());
    assert_eq!(h.fake.count("GetSessionToken"), 0);
}

/// F1-R1: a cached token with more than `TOKEN_REUSE_MARGIN` left is reused, so the start of a
/// session with an MFA device asks no code and does not call AWS.
#[tokio::test]
async fn a_start_with_a_cached_token_asks_no_mfa_code() {
    let (h, clock) = Harness::with_manual_clock().await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some(DEVICE.to_string());
    let id = h.leapp.create_iam_user(req).unwrap();
    h.prompter.mfa_answers(vec![Some("123456"), Some("654321")]);
    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();
    assert_eq!(h.prompter.mfa_asked(), ["dev"]);

    // The token lives 36000 s; 3600 s later it has far more than the margin left.
    clock.advance(jiff::SignedDuration::from_secs(3600));
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.prompter.mfa_asked(), ["dev"], "no second question");
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
}
