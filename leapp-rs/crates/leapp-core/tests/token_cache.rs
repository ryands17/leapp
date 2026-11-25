mod common;

use common::Harness;
use jiff::SignedDuration;
use leapp_core::{SessionKind, SessionStatus, TOKEN_REUSE_MARGIN};

const TOKEN_LIFE: i64 = 36000;

fn secs(n: i64) -> SignedDuration {
    SignedDuration::from_secs(n)
}

/// Create a session, start it, stop it. Returns its id.
async fn started_and_stopped(h: &Harness) -> leapp_core::SessionId {
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();
    id
}

fn account(h: &Harness, id: leapp_core::SessionId) -> Option<String> {
    match &h.leapp.workspace().session(id).unwrap().kind {
        SessionKind::IamUser { account, .. } => account.clone(),
    }
}

#[tokio::test]
async fn a_second_start_reuses_the_token() {
    let (h, _clock) = Harness::with_manual_clock().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
    h.leapp.stop(id).await.unwrap();
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
}

#[tokio::test]
async fn an_expired_token_is_replaced() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started_and_stopped(&h).await;
    clock.advance(secs(TOKEN_LIFE + 1));
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 2);
    assert!(h.credentials_text().unwrap().contains("fake-token-2"));
}

#[tokio::test]
async fn a_token_close_to_expiry_is_replaced() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started_and_stopped(&h).await;
    clock.advance(secs(TOKEN_LIFE - 600));
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 2);
}

#[tokio::test]
async fn a_token_with_time_left_is_reused() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started_and_stopped(&h).await;
    clock.advance(secs(TOKEN_LIFE - 1800));
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 1);
}

#[tokio::test]
async fn the_first_start_stores_the_account() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    assert_eq!(account(&h, id).as_deref(), Some("123456789012"));
    assert_eq!(h.fake.count("GetCallerIdentity"), 1);
    // The call used the temporary credentials, not the long-lived keys.
    let call = h
        .fake
        .requests()
        .into_iter()
        .find(|r| r.action == "GetCallerIdentity")
        .unwrap();
    assert!(call.access_key_id.unwrap().starts_with("ASIAFAKE"));
    // The account is saved on disk.
    assert!(h.workspace_text().contains("123456789012"));

    h.leapp.stop(id).await.unwrap();
    h.leapp.start(id).await.unwrap();
    assert_eq!(h.fake.count("GetCallerIdentity"), 1);
}

#[tokio::test]
async fn a_failed_caller_identity_does_not_fail_the_start() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .fail_next("GetCallerIdentity", 500, "InternalFailure", "boom");
    h.leapp.start(id).await.unwrap();

    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Active
    );
    assert_eq!(account(&h, id), None);
    assert_eq!(h.fake.count("GetCallerIdentity"), 1);
}

/// Starts, stops, then sets the clock to `left` seconds before the stored expiration.
async fn restart_with_time_left(left: i64) -> Harness {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started_and_stopped(&h).await;
    let expiration = h
        .leapp
        .workspace()
        .session(id)
        .unwrap()
        .session_token_expiration
        .unwrap();
    clock.set(expiration.checked_sub(secs(left)).unwrap());
    h.leapp.start(id).await.unwrap();
    h
}

#[tokio::test]
async fn a_token_with_exactly_the_margin_left_is_replaced() {
    let h = restart_with_time_left(TOKEN_REUSE_MARGIN.as_secs()).await;

    assert_eq!(h.fake.count("GetSessionToken"), 2);
}

#[tokio::test]
async fn a_token_with_one_second_more_than_the_margin_is_reused() {
    let h = restart_with_time_left(TOKEN_REUSE_MARGIN.as_secs() + 1).await;

    assert_eq!(h.fake.count("GetSessionToken"), 1);
}
