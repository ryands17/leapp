//! Edit and delete of IAM User sessions (F1-R3, F1-R4, F1-R6, spec 10.3 without the cascade).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{FlakyStore, Harness, block_on};
use leapp_core::{Error, Field, IamUserUpdate, SessionId, SessionKind, SessionStatus};

const MINE: &str = "[default]\naws_access_key_id = MYKEY\naws_secret_access_key = MYSECRET\n";

/// Waits until `done` is true. Panics after 5 s.
async fn settle(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "the condition did not come true");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// An update that keeps every value of the session as it is now.
fn same_as_now(h: &Harness, id: SessionId) -> IamUserUpdate {
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    let SessionKind::IamUser { mfa_device, .. } = &s.kind else {
        panic!("not an IAM User");
    };
    IamUserUpdate {
        name: s.name.clone(),
        profile_name: ws.profile_name(s.profile_id).unwrap().to_string(),
        region: s.region.clone(),
        mfa_device: mfa_device.clone(),
        access_key_id: None,
        secret_access_key: None,
    }
}

fn account(h: &Harness, id: SessionId) -> Option<String> {
    let ws = h.leapp.workspace();
    let SessionKind::IamUser { account, .. } = &ws.session(id).unwrap().kind else {
        panic!("not an IAM User");
    };
    account.clone()
}

fn token_key(id: SessionId) -> String {
    format!("iam-user-{id}-session-token")
}

fn keys_of(h: &Harness, id: SessionId) -> Vec<String> {
    let prefix = format!("iam-user-{id}-");
    h.secrets
        .snapshot()
        .into_keys()
        .filter(|k| k.starts_with(&prefix))
        .collect()
}

#[tokio::test]
async fn edit_with_new_keys_drops_the_token_and_uses_the_new_keys() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();
    assert!(account(&h, id).is_some());
    assert!(h.secrets.snapshot().contains_key(&token_key(id)));

    let mut req = same_as_now(&h, id);
    req.access_key_id = Some("AKID-NEW".into());
    req.secret_access_key = Some("SECRET-NEW".into());
    h.leapp.update_iam_user(id, req).await.unwrap();

    assert!(!h.secrets.snapshot().contains_key(&token_key(id)));
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    assert_eq!(s.session_token_expiration, None);
    assert_eq!(account(&h, id), None);
    assert!(!h.workspace_text().contains("AKID-NEW"));
    assert!(!h.workspace_text().contains("SECRET-NEW"));

    h.leapp.start(id).await.unwrap();
    let last = h
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.action == "GetSessionToken")
        .unwrap();
    assert_eq!(last.access_key_id.as_deref(), Some("AKID-NEW"));
    assert_eq!(h.fake.count("GetSessionToken"), 2);
}

#[tokio::test]
async fn edit_with_empty_keys_keeps_the_old_keys() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();

    let mut req = same_as_now(&h, id);
    req.access_key_id = Some("  ".into());
    req.secret_access_key = None;
    h.leapp.update_iam_user(id, req).await.unwrap();

    let secrets = h.secrets.snapshot();
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-access-key-id"))
            .map(String::as_str),
        Some("AKID-dev")
    );
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-secret-access-key"))
            .map(String::as_str),
        Some("SECRET-dev")
    );
    assert!(secrets.contains_key(&token_key(id)));
    assert!(account(&h, id).is_some());
}

#[tokio::test]
async fn edit_of_an_active_session_moves_its_section() {
    let h = Harness::new().await;
    let id = h.leapp.create_iam_user(h.iam_user("dev", "p-a")).unwrap();
    h.leapp.start(id).await.unwrap();
    assert!(h.credentials_text().unwrap().contains("[p-a]"));

    let mut req = same_as_now(&h, id);
    req.profile_name = "p-b".into();
    h.leapp.update_iam_user(id, req).await.unwrap();

    let text = h.credentials_text().unwrap();
    assert!(!text.contains("[p-a]"), "{text}");
    assert!(text.contains("[p-b]"), "{text}");
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    assert_eq!(s.status, SessionStatus::Active);
    assert_eq!(ws.profile_name(s.profile_id), Some("p-b"));
    assert!(ws.managed_profiles.contains_key("p-b"));
    assert!(!ws.managed_profiles.contains_key("p-a"));
}

#[tokio::test]
async fn edit_of_the_region_of_an_active_session_writes_the_new_region() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    let mut req = same_as_now(&h, id);
    req.region = "us-east-2".into();
    h.leapp.update_iam_user(id, req).await.unwrap();

    let text = h.credentials_text().unwrap();
    assert!(text.contains("us-east-2"), "{text}");
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Active
    );
}

#[tokio::test]
async fn edit_of_only_the_name_does_not_restart() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    let before = h.leapp.workspace().session(id).unwrap().clone();
    let calls = h.fake.requests().len();

    let mut req = same_as_now(&h, id);
    req.name = "renamed".into();
    h.leapp.update_iam_user(id, req).await.unwrap();

    let after = h.leapp.workspace().session(id).unwrap().clone();
    assert_eq!(after.name, "renamed");
    assert_eq!(h.fake.requests().len(), calls);
    assert_eq!(after.start_time, before.start_time);
    assert_eq!(after.status, SessionStatus::Active);
    assert!(h.workspace_text().contains("renamed"));
}

#[tokio::test]
async fn edit_of_the_mfa_device_drops_the_token_and_keeps_the_account() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();

    let mut req = same_as_now(&h, id);
    req.mfa_device = Some(" arn:aws:iam::123456789012:mfa/dev ".into());
    h.leapp.update_iam_user(id, req).await.unwrap();

    let s = h.leapp.workspace().session(id).unwrap().clone();
    let SessionKind::IamUser { mfa_device, .. } = &s.kind else {
        panic!("not an IAM User");
    };
    assert_eq!(
        mfa_device.as_deref(),
        Some("arn:aws:iam::123456789012:mfa/dev")
    );
    assert_eq!(s.session_token_expiration, None);
    assert!(!h.secrets.snapshot().contains_key(&token_key(id)));
    assert_eq!(account(&h, id).as_deref(), Some("123456789012"));
}

#[tokio::test]
async fn edit_of_a_pending_session_is_refused() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (started, edited) = tokio::join!(h.leapp.start(id), async {
        settle(|| h.leapp.workspace().session(id).unwrap().status == SessionStatus::Pending).await;
        let mut req = same_as_now(&h, id);
        req.name = "renamed".into();
        h.leapp.update_iam_user(id, req).await
    });

    started.unwrap();
    assert!(
        matches!(edited, Err(Error::ProfilePending(_))),
        "{edited:?}"
    );
    assert_eq!(h.leapp.workspace().session(id).unwrap().name, "dev");
}

/// A change of one field of an update.
type Change = fn(&mut IamUserUpdate);

#[tokio::test]
async fn edit_validates_like_create() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    let before = h.leapp.workspace();
    let secrets_before = h.secrets.snapshot();

    let cases: [(Field, Change); 4] = [
        (Field::Name, |r| r.name = "  ".into()),
        (Field::Profile, |r| r.profile_name = String::new()),
        (Field::Profile, |r| r.profile_name = "a]b".into()),
        (Field::Region, |r| r.region = "mars-1".into()),
    ];
    for (expected, change) in cases {
        let mut req = same_as_now(&h, id);
        req.access_key_id = Some("AKID-NEW".into());
        change(&mut req);
        let r = h.leapp.update_iam_user(id, req).await;
        match r {
            Err(Error::InvalidInput { field, .. }) => assert_eq!(field, expected),
            other => panic!("expected InvalidInput({expected}), got {other:?}"),
        }
    }
    assert_eq!(h.leapp.workspace(), before);
    assert_eq!(h.secrets.snapshot(), secrets_before);
}

#[tokio::test]
async fn edit_of_an_unknown_session_is_not_found() {
    let h = Harness::new().await;
    let id = uuid::Uuid::new_v4();
    let req = IamUserUpdate {
        name: "x".into(),
        profile_name: "default".into(),
        region: "eu-west-1".into(),
        mfa_device: None,
        access_key_id: None,
        secret_access_key: None,
    };
    let r = h.leapp.update_iam_user(id, req).await;
    assert!(matches!(r, Err(Error::NotFound(x)) if x == id), "{r:?}");
}

#[tokio::test]
async fn update_debug_redacts_both_keys() {
    let req = IamUserUpdate {
        name: "dev".into(),
        profile_name: "default".into(),
        region: "eu-west-1".into(),
        mfa_device: None,
        access_key_id: Some("AKID-SECRETVALUE".into()),
        secret_access_key: Some("SECRET-SECRETVALUE".into()),
    };
    let text = format!("{req:?}");
    assert!(!text.contains("SECRETVALUE"), "{text}");
    assert!(text.contains("dev"), "{text}");
}

/// The account fetch of a start runs after the session is `Active`. An edit of the keys in that
/// window clears the account; the late answer, fetched with the old keys, must not come back.
#[tokio::test]
async fn an_account_fetched_with_the_old_keys_is_not_saved_after_an_edit_of_the_keys() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake.set_account("111111111111");
    h.fake
        .delay_next("GetCallerIdentity", Duration::from_millis(300));

    let (started, edited) = tokio::join!(h.leapp.start(id), async {
        settle(|| h.fake.count("GetCallerIdentity") == 1).await;
        // The fetch of the restart fails, so only the old answer could set the account.
        h.fake
            .fail_next("GetCallerIdentity", 500, "InternalFailure", "boom");
        let mut req = same_as_now(&h, id);
        req.access_key_id = Some("AKID-NEW".into());
        req.secret_access_key = Some("SECRET-NEW".into());
        h.leapp.update_iam_user(id, req).await
    });

    started.unwrap();
    edited.unwrap();
    assert_eq!(account(&h, id), None);
    assert!(!h.workspace_text().contains("111111111111"));
}

/// An edit writes the new keys to the keychain before it saves. A start in that window would
/// read the new access key with the old secret and cache a token that the keychain and the
/// workspace do not show. The edit reserves the session, so such a start is refused.
async fn a_start_while_an_edit_writes_the_keys_is_refused(active: bool) {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    if active {
        h.leapp.start(id).await.unwrap();
    }
    let calls_before = h.fake.count("GetSessionToken");

    // The access key is written; the write of the secret waits at the gate.
    let gate = store.block_set(&format!("iam-user-{id}-secret-access-key"));
    let mut req = same_as_now(&h, id);
    req.access_key_id = Some("AKID-NEW".into());
    req.secret_access_key = Some("SECRET-NEW".into());
    let leapp = &h.leapp;
    let (early_start, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(id, req)));
        gate.wait_blocked();

        let early_start = scope.spawn(|| block_on(leapp.start(id))).join().unwrap();

        gate.open();
        (early_start, editor.join().unwrap())
    });

    assert!(
        matches!(early_start, Err(Error::ProfilePending(_))),
        "{early_start:?}"
    );
    edited.unwrap();
    // The `GetSessionToken` calls after the setup, by access key. The refused start made none.
    let token_calls = || {
        h.fake
            .requests()
            .into_iter()
            .filter(|r| r.action == "GetSessionToken")
            .skip(calls_before)
            .map(|r| r.access_key_id)
            .collect::<Vec<_>>()
    };
    if active {
        // Only the restart of the edit got a token, with the new keys.
        assert_eq!(token_calls(), vec![Some("AKID-NEW".to_string())]);
        assert_eq!(
            h.leapp.workspace().session(id).unwrap().status,
            SessionStatus::Active
        );
    } else {
        assert_eq!(token_calls(), Vec::<Option<String>>::new());
        assert!(!h.secrets.snapshot().contains_key(&token_key(id)));
        assert_eq!(account(&h, id), None);
        h.leapp.start(id).await.unwrap();
        assert_eq!(token_calls(), vec![Some("AKID-NEW".to_string())]);
    }
}

// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_while_an_edit_writes_the_keys_of_an_inactive_session_is_refused() {
    a_start_while_an_edit_writes_the_keys_is_refused(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_while_an_edit_restarts_a_session_with_new_keys_is_refused() {
    a_start_while_an_edit_writes_the_keys_is_refused(true).await;
}

#[tokio::test]
async fn delete_removes_the_session_keys_and_section() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    assert_eq!(keys_of(&h, id).len(), 3);

    h.leapp.delete_session(id).await.unwrap();

    let ws = h.leapp.workspace();
    assert!(ws.session(id).is_none());
    assert!(ws.managed_profiles.is_empty());
    assert!(keys_of(&h, id).is_empty(), "{:?}", keys_of(&h, id));
    assert!(
        h.credentials_text()
            .is_none_or(|t| !t.contains("[default]"))
    );
    assert!(!h.workspace_text().contains(&id.to_string()));
}

#[tokio::test]
async fn delete_puts_the_users_section_back() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    assert_ne!(h.credentials_text().as_deref(), Some(MINE));

    h.leapp.delete_session(id).await.unwrap();

    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(
        !h.secrets
            .snapshot()
            .contains_key("aws-profile-backup-default")
    );
}

#[tokio::test]
async fn delete_of_a_pending_session_is_refused() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (started, deleted) = tokio::join!(h.leapp.start(id), async {
        settle(|| h.leapp.workspace().session(id).unwrap().status == SessionStatus::Pending).await;
        h.leapp.delete_session(id).await
    });

    started.unwrap();
    assert!(
        matches!(deleted, Err(Error::ProfilePending(_))),
        "{deleted:?}"
    );
    assert!(h.leapp.workspace().session(id).is_some());
    assert_eq!(keys_of(&h, id).len(), 3);
}

#[tokio::test]
async fn delete_of_an_unknown_session_is_not_found() {
    let h = Harness::new().await;
    let id = uuid::Uuid::new_v4();
    let r = h.leapp.delete_session(id).await;
    assert!(matches!(r, Err(Error::NotFound(x)) if x == id), "{r:?}");
}

/// A delete while an edit has the session reserved must wait for the edit: else the edit puts
/// the old keys back under the deleted session, and the user's long-lived keys stay in the
/// keychain with no session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_while_an_edit_changes_the_keys_is_refused() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    // The edit reads the old keys without the lock; that read waits at the gate.
    let gate = store.block_get(&format!("iam-user-{id}-access-key-id"));
    let mut req = same_as_now(&h, id);
    req.access_key_id = Some("AKID-NEW".into());
    req.secret_access_key = Some("SECRET-NEW".into());
    let leapp = &h.leapp;
    let (deleted, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(id, req)));
        gate.wait_blocked();

        let deleted = scope
            .spawn(|| block_on(leapp.delete_session(id)))
            .join()
            .unwrap();

        gate.open();
        (deleted, editor.join().unwrap())
    });

    assert!(
        matches!(deleted, Err(Error::ProfilePending(_))),
        "{deleted:?}"
    );
    edited.unwrap();
    assert!(h.leapp.workspace().session(id).is_some());
    let secrets = h.secrets.snapshot();
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-access-key-id"))
            .map(String::as_str),
        Some("AKID-NEW")
    );
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-secret-access-key"))
            .map(String::as_str),
        Some("SECRET-NEW")
    );
}

/// The edit reserves the session after the delete checked for a reservation: while the stop of
/// the delete deletes the user's backup. The delete must see the reservation in the critical
/// section that removes the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_refuses_an_edit_that_began_during_its_stop() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    // The stop of the delete waits at the delete of the backup, after the session is `Inactive`.
    let stop_gate = store.block_delete("aws-profile-backup-default");
    // The edit waits at its read of the old keys, with the session reserved.
    let edit_gate = store.block_get(&format!("iam-user-{id}-access-key-id"));
    let mut req = same_as_now(&h, id);
    req.access_key_id = Some("AKID-NEW".into());
    req.secret_access_key = Some("SECRET-NEW".into());
    let leapp = &h.leapp;
    let (deleted, edited) = std::thread::scope(|scope| {
        let deleter = scope.spawn(|| block_on(leapp.delete_session(id)));
        stop_gate.wait_blocked();
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(id, req)));
        edit_gate.wait_blocked();

        stop_gate.open();
        let deleted = deleter.join().unwrap();
        edit_gate.open();
        (deleted, editor.join().unwrap())
    });

    assert!(
        matches!(deleted, Err(Error::ProfilePending(_))),
        "{deleted:?}"
    );
    edited.unwrap();
    // The stop of the delete completed: the user's section is back.
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(h.leapp.workspace().session(id).is_some());
    assert_eq!(
        h.secrets
            .snapshot()
            .get(&format!("iam-user-{id}-access-key-id"))
            .map(String::as_str),
        Some("AKID-NEW")
    );
}

/// The second key write of an edit fails: the first new key goes out again, the old keys are
/// back, and the session is as it was.
#[tokio::test]
async fn an_edit_whose_key_write_fails_puts_the_old_keys_back() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    let before = h.workspace_text();

    // The write of the new access key works; the write of the new secret fails.
    store.fail_set_after(1);
    let mut req = same_as_now(&h, id);
    req.name = "renamed".into();
    req.access_key_id = Some("AKID-NEW".into());
    req.secret_access_key = Some("SECRET-NEW".into());
    let edited = h.leapp.update_iam_user(id, req).await;

    assert!(matches!(edited, Err(Error::Secret(_))), "{edited:?}");
    let secrets = h.secrets.snapshot();
    let key = |k: &str| secrets.get(&format!("iam-user-{id}-{k}")).cloned();
    assert_eq!(key("access-key-id").as_deref(), Some("AKID-dev"));
    assert_eq!(key("secret-access-key").as_deref(), Some("SECRET-dev"));
    assert_eq!(h.workspace_text(), before);
    assert_eq!(h.leapp.workspace().session(id).unwrap().name, "dev");
    // The session still starts, with the old keys.
    h.leapp.start(id).await.unwrap();
    let call = h.fake.requests().into_iter().next().unwrap();
    assert_eq!(call.access_key_id.as_deref(), Some("AKID-dev"));
}
