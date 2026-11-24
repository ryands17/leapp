//! Gaps that the F0 reviews found: shutdown during a start, lock scope, failed secret writes,
//! missing backups and empty AWS messages.

mod common;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{FlakyStore, Harness};
use leapp_core::{Error, SecretStore, SessionStatus};

const MINE: &str = "[default]\naws_access_key_id = MYKEY\naws_secret_access_key = MYSECRET\n";
const BACKUP_KEY: &str = "aws-profile-backup-default";

/// Waits until `done` is true. Panics after 5 s.
async fn settle(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "the condition did not come true");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_start_that_waits_on_aws_at_shutdown_leaves_no_backup() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (started, shut) = tokio::join!(h.leapp.start(id), async {
        settle(|| h.fake.count("GetSessionToken") == 1).await;
        h.leapp.shutdown().await
    });

    shut.unwrap();
    assert!(matches!(started, Err(Error::Closing)), "{started:?}");
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(
        !h.secrets.snapshot().contains_key(BACKUP_KEY),
        "{:?}",
        h.secrets.snapshot().keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_start_after_shutdown_makes_no_sts_call() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.shutdown().await.unwrap();
    let started = h.leapp.start(id).await;

    assert!(matches!(started, Err(Error::Closing)), "{started:?}");
    assert_eq!(h.fake.count("GetSessionToken"), 0);
}

#[tokio::test]
async fn a_stop_does_not_hold_the_lock_while_the_keychain_reads() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    let gate = store.block_get("aws-profile-backup-");
    let leapp = &h.leapp;
    let (read_in_time, stopped) = std::thread::scope(|scope| {
        let stopper = scope.spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(leapp.stop(id))
        });
        gate.wait_blocked();

        let (tx, rx) = mpsc::channel();
        let reader = scope.spawn(move || {
            let _ = tx.send(leapp.workspace());
        });
        let read_in_time = rx.recv_timeout(Duration::from_millis(200)).is_ok();

        gate.open();
        reader.join().unwrap();
        (read_in_time, stopper.join().unwrap())
    });

    assert!(
        read_in_time,
        "workspace() waited on the lock while the stop read the keychain"
    );
    stopped.unwrap();
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
}

/// Runs `fut` on a new runtime on this thread.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_while_a_stop_deletes_the_backup_waits_its_turn() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let a = h
        .leapp
        .create_iam_user(h.iam_user("dev-a", "default"))
        .unwrap();
    let b = h
        .leapp
        .create_iam_user(h.iam_user("dev-b", "default"))
        .unwrap();
    h.leapp.start(a).await.unwrap();

    let gate = store.block_delete("aws-profile-backup-");
    let leapp = &h.leapp;
    let (early_start, stopped) = std::thread::scope(|scope| {
        let stopper = scope.spawn(|| block_on(leapp.stop(a)));
        gate.wait_blocked();

        let early_start = scope.spawn(|| block_on(leapp.start(b))).join().unwrap();

        gate.open();
        (early_start, stopper.join().unwrap())
    });

    assert!(
        matches!(early_start, Err(Error::ProfilePending(_))),
        "{early_start:?}"
    );
    stopped.unwrap();
    h.leapp.start(b).await.unwrap();
    h.leapp.stop(b).await.unwrap();
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
}

#[tokio::test]
async fn a_failed_second_secret_write_leaves_no_session_and_no_keys() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    store.fail_set_after(1);

    let created = h.leapp.create_iam_user(h.iam_user("dev", "default"));

    assert!(matches!(created, Err(Error::Secret(_))), "{created:?}");
    assert!(h.leapp.workspace().sessions.is_empty());
    let keys: Vec<String> = h.secrets.snapshot().into_keys().collect();
    assert!(!keys.iter().any(|k| k.starts_with("iam-user-")), "{keys:?}");
}

#[tokio::test]
async fn a_stop_with_a_missing_backup_says_so() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    h.secrets.delete(BACKUP_KEY).unwrap();

    let stopped = h.leapp.stop(id).await;

    match &stopped {
        Err(e @ Error::BackupMissing(p)) => {
            assert_eq!(p, "default");
            assert_eq!(
                e.to_string(),
                "the saved copy of profile \"default\" is missing from the keychain item \
                 \"aws-profile-backup-default\" (service leapp-rs). Leapp RS removed its own keys \
                 from the profile, but could not put your keys back."
            );
        }
        other => panic!("expected BackupMissing, got {other:?}"),
    }
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(id).unwrap().status, SessionStatus::Inactive);
    assert!(!h.credentials_text().unwrap().contains("[default]"));
    assert!(ws.managed_profiles.is_empty());
}

#[tokio::test]
async fn an_aws_error_with_an_empty_message_shows_the_full_error() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake.fail_next("GetSessionToken", 403, "AccessDenied", "");

    let started = h.leapp.start(id).await;

    match started {
        Err(Error::Aws(m)) => assert!(!m.is_empty()),
        other => panic!("expected Error::Aws, got {other:?}"),
    }
}
