//! Refresh of old active sessions (spec 10.4, F1-R2, F1-R7).

mod common;

use std::sync::Arc;

use common::{FlakyStore, Harness};
use jiff::SignedDuration;
use leapp_core::{
    IamUserUpdate, REFRESH_TICK, SESSION_DURATION, SessionId, SessionKind, SessionStatus,
};

const DEVICE: &str = "arn:aws:iam::123456789012:mfa/dev";
const MINE: &str = "[default]\naws_access_key_id = MYKEY\naws_secret_access_key = MYSECRET\n";

fn secs(n: i64) -> SignedDuration {
    SignedDuration::from_secs(n)
}

async fn started(h: &Harness) -> SessionId {
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    id
}

fn status(h: &Harness, id: SessionId) -> SessionStatus {
    h.leapp.workspace().session(id).unwrap().status
}

#[test]
fn the_timings_are_the_spec_values() {
    assert_eq!(REFRESH_TICK, std::time::Duration::from_secs(10));
    assert_eq!(SESSION_DURATION, secs(1200));
}

#[tokio::test]
async fn a_young_session_is_not_refreshed() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started(&h).await;
    let before = h.credentials_text();
    let start_time = h.leapp.workspace().session(id).unwrap().start_time;
    clock.advance(secs(1200));

    let report = h.leapp.refresh_tick().await;

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(h.credentials_text(), before);
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().start_time,
        start_time
    );
}

#[tokio::test]
async fn an_old_session_is_written_again_with_the_cached_token() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started(&h).await;
    clock.advance(secs(21 * 60));
    let now = leapp_core::Clock::now(&*clock);

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.refreshed, vec![id]);
    assert!(report.failures.is_empty());
    assert!(!report.is_empty());
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    assert_eq!(s.start_time, Some(now));
    assert_eq!(s.status, SessionStatus::Active);
    // F1-R2: the token is reused, so AWS gets no second call.
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
    // The new start time is saved on disk too.
    assert!(h.workspace_text().contains(&now.to_string()[..19]));
}

#[tokio::test]
async fn an_old_session_with_an_expired_token_gets_new_credentials() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started(&h).await;
    clock.advance(secs(36001));

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.refreshed, vec![id]);
    assert!(h.credentials_text().unwrap().contains("fake-token-2"));
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(h.fake.count("GetSessionToken"), 2);
}

#[tokio::test]
async fn a_refresh_error_stops_the_session() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started(&h).await;
    clock.advance(secs(36001));
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "ExpiredToken",
        "The security token expired",
    );

    let report = h.leapp.refresh_tick().await;

    assert!(report.refreshed.is_empty());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    let failure = &report.failures[0];
    assert_eq!(failure.session_id, id);
    assert_eq!(failure.session_name, "dev");
    assert!(
        failure
            .error
            .to_string()
            .contains("The security token expired"),
        "{}",
        failure.error
    );
    assert_eq!(status(&h, id), SessionStatus::Inactive);
    assert!(
        !h.credentials_text()
            .unwrap_or_default()
            .contains("[default]")
    );
    assert!(h.leapp.workspace().managed_profiles.is_empty());
    assert!(!h.workspace_text().contains("\"active\""));
}

#[tokio::test]
async fn a_refresh_error_puts_the_users_section_back() {
    let (h, clock) = Harness::with_manual_clock().await;
    h.write_credentials(MINE);
    let id = started(&h).await;
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
    clock.advance(secs(36001));
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "ExpiredToken",
        "The security token expired",
    );

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert_eq!(status(&h, id), SessionStatus::Inactive);
    assert!(
        !h.secrets
            .snapshot()
            .contains_key("aws-profile-backup-default")
    );
}

#[tokio::test]
async fn a_stopped_session_is_not_refreshed() {
    let (h, clock) = Harness::with_manual_clock().await;
    let id = started(&h).await;
    h.leapp.stop(id).await.unwrap();
    clock.advance(secs(21 * 60));

    let report = h.leapp.refresh_tick().await;

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert!(
        !h.credentials_text()
            .unwrap_or_default()
            .contains("[default]")
    );
    assert_eq!(status(&h, id), SessionStatus::Inactive);
}

#[tokio::test]
async fn a_cancelled_mfa_on_refresh_stops_the_session() {
    let (h, clock) = Harness::with_manual_clock().await;
    let mut req = h.iam_user("dev", "default");
    req.mfa_device = Some(DEVICE.to_string());
    let id = h.leapp.create_iam_user(req).unwrap();
    h.prompter.mfa_answers(vec![Some("123456"), None]);
    h.leapp.start(id).await.unwrap();
    clock.advance(secs(36001));

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].error.to_string(),
        "Missing Multi Factor Authentication code"
    );
    assert_eq!(h.prompter.mfa_asked(), ["dev", "dev"]);
    assert_eq!(status(&h, id), SessionStatus::Inactive);
    assert!(
        !h.credentials_text()
            .unwrap_or_default()
            .contains("[default]")
    );
    assert_eq!(h.fake.count("GetSessionToken"), 1);
}

#[tokio::test]
async fn a_tick_after_shutdown_does_nothing() {
    let (h, clock) = Harness::with_manual_clock().await;
    let _id = started(&h).await;
    clock.advance(secs(21 * 60));
    h.leapp.shutdown().await.unwrap();
    let before = h.credentials_text();

    let report = h.leapp.refresh_tick().await;

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(h.credentials_text(), before);
}

#[tokio::test]
async fn sessions_are_refreshed_one_after_another() {
    let (h, clock) = Harness::with_manual_clock().await;
    let a = started(&h).await;
    let b = h.leapp.create_iam_user(h.iam_user("ops", "work")).unwrap();
    h.leapp.start(b).await.unwrap();
    clock.advance(secs(36001));
    // The first refresh fails; the second still runs.
    h.fake
        .fail_next("GetSessionToken", 500, "InternalFailure", "boom");

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].session_id, a);
    assert_eq!(report.refreshed, vec![b]);
    assert_eq!(status(&h, a), SessionStatus::Inactive);
    assert_eq!(status(&h, b), SessionStatus::Active);
}

/// Runs `fut` on a new runtime on this thread.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

/// An edit reserves the session while it reads and writes keys. A refresh then would set
/// `Pending` under the edit, so it skips the session this tick (no failure).
// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_under_edit_is_skipped() {
    let store = Arc::new(FlakyStore::default());
    let (h, clock) = Harness::with_store_and_manual_clock(store.clone()).await;
    let id = started(&h).await;
    clock.advance(secs(21 * 60));
    let before = h.credentials_text();

    // A name-only edit that gives the same keys: it reads the old keys under the reservation.
    let gate = store.block_get(&format!("iam-user-{id}-access-key-id"));
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    let SessionKind::IamUser { mfa_device, .. } = &s.kind;
    let req = IamUserUpdate {
        name: "dev2".into(),
        profile_name: "default".into(),
        region: s.region.clone(),
        mfa_device: mfa_device.clone(),
        access_key_id: Some("AKID-dev".into()),
        secret_access_key: Some("SECRET-dev".into()),
    };
    let leapp = &h.leapp;
    let (report, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(id, req)));
        gate.wait_blocked();
        let report = scope
            .spawn(|| block_on(leapp.refresh_tick()))
            .join()
            .unwrap();
        gate.open();
        (report, editor.join().unwrap())
    });

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(h.credentials_text(), before);
    edited.unwrap();
    assert_eq!(status(&h, id), SessionStatus::Active);

    // The next tick refreshes it.
    let report = h.leapp.refresh_tick().await;
    assert_eq!(report.refreshed, vec![id]);
}

/// A second tick while a refresh waits on the keychain finds the session `Pending` and skips it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_in_refresh_is_not_refreshed_again() {
    let store = Arc::new(FlakyStore::default());
    let (h, clock) = Harness::with_store_and_manual_clock(store.clone()).await;
    let id = started(&h).await;
    clock.advance(secs(21 * 60));

    let gate = store.block_get(&format!("iam-user-{id}-session-token"));
    let leapp = &h.leapp;
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| block_on(leapp.refresh_tick()));
        gate.wait_blocked();
        let (tx, rx) = std::sync::mpsc::channel();
        let second = scope.spawn(move || tx.send(block_on(leapp.refresh_tick())).unwrap());
        // A second refresh of the session would wait at the closed gate too.
        let report = rx.recv_timeout(std::time::Duration::from_secs(5));
        gate.open();
        second.join().unwrap();
        (first.join().unwrap(), report)
    });

    let second = second.expect("the second tick waited for the first refresh");
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(first.refreshed, vec![id]);
    assert_eq!(status(&h, id), SessionStatus::Active);
}
